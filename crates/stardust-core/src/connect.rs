//! Login-only connect: the RFC 8628 device flow, one request at a time.
//!
//! The check timer drives a [`DeviceFlow`]: it asks how long to wait, waits
//! on its own timer, then calls [`DeviceFlow::step`] for one poll. Nothing
//! here sleeps or starts a thread. The interval from the server is honoured,
//! `slow_down` adds 5 seconds for this and every later poll, `expired_token`
//! and `access_denied` end the flow, and a network error backs off with
//! jitter without passing the code's expiry.

use crate::api::{
    verification_url, DeviceCodeResponse, DevicePoll, DeviceTokenResponse, OrbitError,
    DEVICE_CODE_TTL_SECONDS,
};
use crate::check::full_jitter;
use crate::i18n::{tr, Language, Msg};
use crate::pet::ObservedCheck;
use std::fmt;
use std::time::{Duration, Instant};

/// Longest poll interval: the longest code lifetime. The server's interval
/// and every `slow_down` step are kept as asked; only the code's expiry
/// bounds them.
pub const MAX_POLL_INTERVAL: Duration = Duration::from_secs(DEVICE_CODE_TTL_SECONDS);
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

/// The two device-grant calls. [`crate::http::HttpOrbitClient`] is the real one.
pub trait DeviceGrant {
    fn start_device_code(&self) -> Result<DeviceCodeResponse, OrbitError>;
    fn poll_device_token(&self, device_code: &str) -> Result<DevicePoll, OrbitError>;
}

impl DeviceGrant for crate::http::HttpOrbitClient {
    fn start_device_code(&self) -> Result<DeviceCodeResponse, OrbitError> {
        crate::http::HttpOrbitClient::start_device_code(self)
    }

    fn poll_device_token(&self, device_code: &str) -> Result<DevicePoll, OrbitError> {
        crate::http::HttpOrbitClient::poll_device_token(self, device_code)
    }
}

/// What the tray says about the ORBIT connection when auto sign-in is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectStatus {
    Connecting,
    Connected,
    SignedOut,
    Offline,
}

impl ConnectStatus {
    pub fn label(self, language: Language) -> &'static str {
        tr(
            language,
            match self {
                Self::Connecting => Msg::ConnectConnecting,
                Self::Connected => Msg::ConnectConnected,
                Self::SignedOut => Msg::ConnectSignedOut,
                Self::Offline => Msg::ConnectOffline,
            },
        )
    }
}

/// Where the device flow stands, as the tray shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FlowPhase {
    #[default]
    Idle,
    /// Waiting for the browser. The user code is not a secret.
    Waiting { user_code: String },
    /// The last poll did not reach ORBIT. Polling continues with backoff.
    Offline { user_code: String },
}

/// Tray status from the flow phase, whether a token is saved, and the last
/// check. Before the first check a saved token reads as connected and no
/// token as signed out.
pub fn connect_status(
    phase: &FlowPhase,
    observed: ObservedCheck,
    signed_in: bool,
) -> ConnectStatus {
    match phase {
        FlowPhase::Waiting { .. } => ConnectStatus::Connecting,
        FlowPhase::Offline { .. } => ConnectStatus::Offline,
        FlowPhase::Idle if !signed_in => ConnectStatus::SignedOut,
        FlowPhase::Idle => match observed {
            ObservedCheck::SignedOut => ConnectStatus::SignedOut,
            ObservedCheck::Unavailable | ObservedCheck::Failed => ConnectStatus::Offline,
            ObservedCheck::Waiting | ObservedCheck::Succeeded { .. } => ConnectStatus::Connected,
        },
    }
}

/// The tray line: the status, and the user code while connecting so it can
/// be typed in when no browser opened.
pub fn status_line(
    phase: &FlowPhase,
    observed: ObservedCheck,
    signed_in: bool,
    language: Language,
) -> String {
    let label = connect_status(phase, observed, signed_in).label(language);
    match phase {
        FlowPhase::Waiting { user_code } | FlowPhase::Offline { user_code } => {
            format!("{label} {user_code}")
        }
        FlowPhase::Idle => label.to_string(),
    }
}

/// One step of the flow.
#[derive(Debug, PartialEq, Eq)]
pub enum FlowStep {
    /// Too early to poll. Come back after this long.
    Wait(Duration),
    /// The browser has not approved yet. Next poll after this long.
    Pending(Duration),
    /// The poll did not reach ORBIT. Next poll after this backoff.
    Offline(Duration),
    Approved(DeviceTokenResponse),
    /// Expired, denied, or refused. The flow is over until the user starts
    /// it again.
    Ended(OrbitError),
}

pub struct DeviceFlow {
    device_code: String,
    user_code: String,
    verification: Option<String>,
    interval: Duration,
    next_poll: Instant,
    deadline: Instant,
    failures: u32,
}

impl fmt::Debug for DeviceFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceFlow")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("interval", &self.interval)
            .field("failures", &self.failures)
            .finish()
    }
}

impl DeviceFlow {
    /// Ask ORBIT for a device code. The first poll waits one interval.
    ///
    /// `clock` is read after the answer arrives, so a slow request does not
    /// bring the first poll forward or the expiry back.
    pub fn start(
        client: &dyn DeviceGrant,
        server_base: &str,
        clock: impl FnOnce() -> Instant,
    ) -> Result<Self, OrbitError> {
        let started = client.start_device_code()?;
        let now = clock();
        let interval = Duration::from_secs(started.interval.max(1)).min(MAX_POLL_INTERVAL);
        let lifetime = Duration::from_secs(started.expires_in.clamp(1, DEVICE_CODE_TTL_SECONDS));
        Ok(Self {
            verification: verification_url(server_base, &started),
            device_code: started.device_code,
            user_code: started.user_code,
            interval,
            next_poll: now + interval,
            deadline: now + lifetime,
            failures: 0,
        })
    }

    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    /// `verification_uri_complete` (or `verification_uri`) when it is https
    /// on the configured host. Nothing else is opened.
    pub fn verification_url(&self) -> Option<&str> {
        self.verification.as_deref()
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Time until the next poll, or until the code expires.
    pub fn wait(&self, now: Instant) -> Duration {
        self.next_poll
            .min(self.deadline)
            .saturating_duration_since(now)
    }

    pub fn step(&mut self, client: &dyn DeviceGrant, now: Instant, entropy: u64) -> FlowStep {
        if now >= self.deadline {
            return FlowStep::Ended(OrbitError::Expired);
        }
        if now < self.next_poll {
            return FlowStep::Wait(self.next_poll - now);
        }
        match client.poll_device_token(&self.device_code) {
            Ok(DevicePoll::Approved(token)) => FlowStep::Approved(token),
            Ok(DevicePoll::Pending) => {
                self.failures = 0;
                FlowStep::Pending(self.schedule(now, self.interval))
            }
            Ok(DevicePoll::SlowDown { interval }) => {
                self.failures = 0;
                let reported = Duration::from_secs(interval);
                self.interval = (self.interval + SLOW_DOWN_STEP)
                    .max(reported)
                    .min(MAX_POLL_INTERVAL);
                FlowStep::Pending(self.schedule(now, self.interval))
            }
            Err(OrbitError::Unreachable) => {
                self.failures = self.failures.saturating_add(1);
                let backoff = self.interval.max(full_jitter(self.failures, entropy));
                FlowStep::Offline(self.schedule(now, backoff))
            }
            Err(err) => FlowStep::Ended(err),
        }
    }

    fn schedule(&mut self, now: Instant, delay: Duration) -> Duration {
        self.next_poll = (now + delay).min(self.deadline);
        self.next_poll.saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        connect_status, status_line, ConnectStatus, DeviceFlow, FlowPhase, FlowStep,
        MAX_POLL_INTERVAL,
    };
    use crate::api::{OrbitError, PATH_DEVICE_CODE, PATH_DEVICE_TOKEN};
    use crate::http::HttpOrbitClient;
    use crate::i18n::Language;
    use crate::pet::ObservedCheck;
    use crate::testserver::{Reply, TestServer};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const DEVICE_CODE: &str = "abababababababababababababababababababababababababababababababab";

    fn code_reply(interval: u64, expires_in: u64) -> Reply {
        Reply::json(
            200,
            &format!(
                r#"{{"device_code":"{DEVICE_CODE}","user_code":"ABCD-EFGH","verification_uri":"https://helloorbit.app/rust-preview/pqc/account","verification_uri_complete":"https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH","expires_in":{expires_in},"interval":{interval}}}"#
            ),
        )
    }

    fn token_reply() -> Reply {
        Reply::json(
            200,
            r#"{"token":"example-access","access_token":"example-access","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":1893456000000}"#,
        )
    }

    /// A server that answers device-token polls from a script.
    fn scripted(interval: u64, expires_in: u64, polls: Vec<Reply>) -> TestServer {
        let polls = Arc::new(Mutex::new(polls.into_iter()));
        TestServer::start(move |request| match request.path() {
            PATH_DEVICE_CODE => code_reply(interval, expires_in),
            PATH_DEVICE_TOKEN => polls
                .lock()
                .unwrap()
                .next()
                .unwrap_or_else(|| Reply::json(400, r#"{"error":"authorization_pending"}"#)),
            _ => Reply::empty(404),
        })
    }

    fn client(server: &TestServer, base_override: Option<&str>) -> HttpOrbitClient {
        HttpOrbitClient::with_timeout(
            base_override.unwrap_or(&server.base()),
            Duration::from_secs(2),
        )
        .unwrap()
    }

    #[test]
    fn pending_then_slow_down_then_approval_follows_the_server_interval() {
        let server = scripted(
            5,
            600,
            vec![
                Reply::json(400, r#"{"error":"authorization_pending"}"#),
                Reply::json(400, r#"{"error":"slow_down"}"#),
                Reply::json(400, r#"{"error":"slow_down","interval":20}"#),
                Reply::json(400, r#"{"error":"authorization_pending"}"#),
                token_reply(),
            ],
        );
        let client = client(&server, None);
        let t0 = Instant::now();
        let mut flow = DeviceFlow::start(&client, &server.base(), || t0).unwrap();
        assert_eq!(flow.user_code(), "ABCD-EFGH");
        assert_eq!(
            flow.verification_url(),
            None,
            "a plain-http server never opens a page"
        );
        assert!(!format!("{flow:?}").contains(DEVICE_CODE));
        assert_eq!(flow.wait(t0), Duration::from_secs(5));
        assert_eq!(
            flow.step(&client, t0, 0),
            FlowStep::Wait(Duration::from_secs(5))
        );
        assert_eq!(
            server.count(PATH_DEVICE_TOKEN),
            0,
            "no poll before the interval"
        );
        let mut now = t0 + Duration::from_secs(5);
        assert_eq!(
            flow.step(&client, now, 0),
            FlowStep::Pending(Duration::from_secs(5))
        );
        now += Duration::from_secs(5);
        assert_eq!(
            flow.step(&client, now, 0),
            FlowStep::Pending(Duration::from_secs(10)),
            "slow_down adds 5 seconds"
        );
        now += Duration::from_secs(10);
        assert_eq!(
            flow.step(&client, now, 0),
            FlowStep::Pending(Duration::from_secs(20)),
            "a larger interval from the server wins"
        );
        now += Duration::from_secs(20);
        assert_eq!(
            flow.step(&client, now, 0),
            FlowStep::Pending(Duration::from_secs(20)),
            "the longer interval stays for later polls"
        );
        now += Duration::from_secs(20);
        let FlowStep::Approved(token) = flow.step(&client, now, 0) else {
            panic!("expected approval");
        };
        assert_eq!(token.access_token, "example-access");
        assert_eq!(server.count(PATH_DEVICE_TOKEN), 5);
        let bodies: Vec<String> = server
            .requests()
            .into_iter()
            .filter(|request| request.path() == PATH_DEVICE_TOKEN)
            .map(|request| request.body)
            .collect();
        assert!(bodies.iter().all(|body| body.contains(DEVICE_CODE)
            && body.contains("urn:ietf:params:oauth:grant-type:device_code")));
    }

    #[test]
    fn a_long_server_interval_is_kept_and_only_the_expiry_cuts_it() {
        let server = scripted(
            50,
            600,
            vec![
                Reply::json(400, r#"{"error":"slow_down","interval":30}"#),
                Reply::json(400, r#"{"error":"slow_down"}"#),
                Reply::json(400, r#"{"error":"slow_down"}"#),
            ],
        );
        let polling = client(&server, None);
        let t0 = Instant::now();
        let mut flow = DeviceFlow::start(&polling, &server.base(), || t0).unwrap();
        let mut now = t0 + Duration::from_secs(50);
        for expected in [55, 60, 65] {
            assert_eq!(
                flow.step(&polling, now, 0),
                FlowStep::Pending(Duration::from_secs(expected)),
                "each slow_down adds 5 seconds, past a minute too"
            );
            now += Duration::from_secs(expected);
        }
        assert_eq!(flow.interval(), Duration::from_secs(65));

        let slow = scripted(900, 600, Vec::new());
        let slow_client = client(&slow, None);
        let flow = DeviceFlow::start(&slow_client, &slow.base(), || t0).unwrap();
        assert_eq!(flow.interval(), MAX_POLL_INTERVAL);
        assert_eq!(
            flow.wait(t0),
            Duration::from_secs(600),
            "never sooner than the server asked; the code expires first"
        );
    }

    #[test]
    fn the_first_poll_is_timed_from_the_answer_not_the_request() {
        let server = scripted(5, 600, Vec::new());
        let client = client(&server, None);
        let asked = Instant::now();
        let answered = asked + Duration::from_secs(3);
        let flow = DeviceFlow::start(&client, &server.base(), || answered).unwrap();
        assert_eq!(flow.wait(asked), Duration::from_secs(8));
        assert_eq!(flow.wait(answered), Duration::from_secs(5));
    }

    #[test]
    fn expired_token_and_access_denied_end_the_flow() {
        let server = scripted(
            1,
            600,
            vec![Reply::json(400, r#"{"error":"expired_token"}"#)],
        );
        let expired_client = client(&server, None);
        let t0 = Instant::now();
        let mut flow = DeviceFlow::start(&expired_client, &server.base(), || t0).unwrap();
        assert_eq!(
            flow.step(&expired_client, t0 + Duration::from_secs(1), 0),
            FlowStep::Ended(OrbitError::Expired)
        );
        let denied = scripted(
            1,
            600,
            vec![Reply::json(400, r#"{"error":"access_denied"}"#)],
        );
        let denied_client = client(&denied, None);
        let mut flow = DeviceFlow::start(&denied_client, &denied.base(), || t0).unwrap();
        assert_eq!(
            flow.step(&denied_client, t0 + Duration::from_secs(1), 0),
            FlowStep::Ended(OrbitError::Denied)
        );
    }

    #[test]
    fn the_code_lifetime_ends_the_flow_without_another_poll() {
        let server = scripted(5, 12, Vec::new());
        let client = client(&server, None);
        let t0 = Instant::now();
        let mut flow = DeviceFlow::start(&client, &server.base(), || t0).unwrap();
        assert_eq!(
            flow.step(&client, t0 + Duration::from_secs(5), 0),
            FlowStep::Pending(Duration::from_secs(5))
        );
        assert_eq!(
            flow.step(&client, t0 + Duration::from_secs(10), 0),
            FlowStep::Pending(Duration::from_secs(2)),
            "the next poll is cut to the expiry"
        );
        assert_eq!(
            flow.wait(t0 + Duration::from_secs(11)),
            Duration::from_secs(1)
        );
        assert_eq!(
            flow.step(&client, t0 + Duration::from_secs(12), 0),
            FlowStep::Ended(OrbitError::Expired)
        );
        assert_eq!(server.count(PATH_DEVICE_TOKEN), 2);
    }

    #[test]
    fn a_network_error_backs_off_with_jitter_and_does_not_spin() {
        let server = scripted(5, 600, Vec::new());
        let reachable = client(&server, None);
        let t0 = Instant::now();
        let mut flow = DeviceFlow::start(&reachable, &server.base(), || t0).unwrap();
        let closed =
            HttpOrbitClient::with_timeout("http://127.0.0.1:1", Duration::from_millis(500))
                .unwrap();
        let mut now = t0 + Duration::from_secs(5);
        let FlowStep::Offline(first) = flow.step(&closed, now, 0) else {
            panic!("expected a backoff");
        };
        assert_eq!(
            first,
            Duration::from_secs(5),
            "never shorter than the interval"
        );
        now += first;
        let FlowStep::Offline(second) = flow.step(&closed, now, 59_000) else {
            panic!("expected a backoff");
        };
        assert_eq!(
            second,
            Duration::from_secs(59),
            "jitter under a 60 s ceiling"
        );
        assert_eq!(
            flow.step(&closed, now + Duration::from_secs(1), 0),
            FlowStep::Wait(Duration::from_secs(58)),
            "no poll before the backoff ends"
        );
        now += second;
        assert_eq!(
            flow.step(&reachable, now, 0),
            FlowStep::Pending(Duration::from_secs(5)),
            "a reachable poll resets the backoff"
        );
        assert_eq!(server.count(PATH_DEVICE_TOKEN), 1);
    }

    #[test]
    fn the_browser_page_must_be_https_on_the_configured_host() {
        let server = scripted(5, 600, Vec::new());
        let client = client(&server, None);
        let t0 = Instant::now();
        let flow = DeviceFlow::start(&client, "https://helloorbit.app", || t0).unwrap();
        assert_eq!(
            flow.verification_url(),
            Some("https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH")
        );
        let other = DeviceFlow::start(&client, "https://other.example", || t0).unwrap();
        assert_eq!(other.verification_url(), None);
        let off = TestServer::start(|_| Reply::empty(404));
        let off_client =
            HttpOrbitClient::with_timeout(&off.base(), Duration::from_secs(2)).unwrap();
        assert_eq!(
            DeviceFlow::start(&off_client, &off.base(), || t0).unwrap_err(),
            OrbitError::FeatureOff
        );
    }

    #[test]
    fn tray_status_follows_the_flow_then_the_last_check() {
        let waiting = FlowPhase::Waiting {
            user_code: "ABCD-EFGH".to_string(),
        };
        assert_eq!(
            connect_status(&waiting, ObservedCheck::SignedOut, false),
            ConnectStatus::Connecting
        );
        assert_eq!(
            connect_status(
                &FlowPhase::Offline {
                    user_code: "ABCD-EFGH".to_string()
                },
                ObservedCheck::Waiting,
                false
            ),
            ConnectStatus::Offline
        );
        assert_eq!(
            connect_status(&FlowPhase::Idle, ObservedCheck::SignedOut, true),
            ConnectStatus::SignedOut
        );
        assert_eq!(
            connect_status(&FlowPhase::Idle, ObservedCheck::Waiting, false),
            ConnectStatus::SignedOut,
            "no token before the first check is signed out, not connected"
        );
        assert_eq!(
            connect_status(&FlowPhase::Idle, ObservedCheck::Waiting, true),
            ConnectStatus::Connected
        );
        assert_eq!(
            connect_status(&FlowPhase::Idle, ObservedCheck::Unavailable, true),
            ConnectStatus::Offline
        );
        assert_eq!(
            connect_status(
                &FlowPhase::Idle,
                ObservedCheck::Succeeded {
                    item_count: 0,
                    high_severity: false
                },
                true
            ),
            ConnectStatus::Connected
        );
        assert_eq!(
            status_line(&waiting, ObservedCheck::Waiting, false, Language::English),
            "ORBIT: Connecting... ABCD-EFGH"
        );
        assert_eq!(
            status_line(
                &FlowPhase::Idle,
                ObservedCheck::Unavailable,
                true,
                Language::Korean
            ),
            "ORBIT: 오프라인"
        );
    }
}
