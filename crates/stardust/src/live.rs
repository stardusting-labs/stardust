//! State shared by the tray, the windows, and the check timer: the latest
//! check, the monitor scale, and, with the newer features on, the ORBIT
//! connection and the requests the check timer carries out on its next tick.

use chrono::{DateTime, Utc};
use stardust_core::connect::FlowPhase;
use stardust_core::pet::ObservedCheck;
use std::fmt;
use std::sync::{Arc, Mutex};

/// "Send test" requests waiting at once. More clicks are dropped.
const MAX_PENDING_TESTS: usize = 16;

#[derive(Clone)]
pub struct LivePet {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    observed: ObservedCheck,
    scale_factor: f64,
    phase: FlowPhase,
    signed_in: Option<bool>,
    connect_requested: bool,
    tests: Vec<String>,
    sign_out: Option<SignOutTicket>,
    /// The fields below are used only with the launch sign-in window on.
    cancel_requested: bool,
    attempt: u64,
    session_until: Option<DateTime<Utc>>,
    sign_in_window_open: bool,
    demo: bool,
}

/// What the check timer should do about the device sign-in on this tick. It
/// handles `cancel` first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowRequests {
    pub cancel: bool,
    /// Start a flow, stamped with this attempt number.
    pub connect: Option<u64>,
}

/// A sign-out from the settings window, finished by the check timer: it
/// deletes the keychain record again in case a running check saved one, and
/// asks ORBIT to revoke the refresh token when there was one. Debug output
/// never shows the token.
pub struct SignOutTicket(Option<String>);

impl SignOutTicket {
    pub fn refresh_token(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl fmt::Debug for SignOutTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = self.0.as_ref().map(|_| "<redacted>");
        formatter
            .debug_tuple("SignOutTicket")
            .field(&shown)
            .finish()
    }
}

impl LivePet {
    pub fn new(scale_factor: f64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                observed: ObservedCheck::Waiting,
                scale_factor: finite_scale(scale_factor),
                phase: FlowPhase::Idle,
                signed_in: None,
                connect_requested: false,
                tests: Vec::new(),
                sign_out: None,
                cancel_requested: false,
                attempt: 0,
                session_until: None,
                sign_in_window_open: false,
                demo: false,
            })),
        }
    }

    pub fn observed(&self) -> ObservedCheck {
        self.lock().observed
    }

    pub fn set_observed(&self, observed: ObservedCheck) {
        self.lock().observed = observed;
    }

    pub fn scale_factor(&self) -> f64 {
        self.lock().scale_factor
    }

    pub fn set_scale_factor(&self, scale_factor: f64) {
        if scale_factor.is_finite() && scale_factor > 0.0 {
            self.lock().scale_factor = scale_factor;
        }
    }

    /// Where the device sign-in on the check timer stands.
    pub fn phase(&self) -> FlowPhase {
        self.lock().phase.clone()
    }

    pub fn set_phase(&self, phase: FlowPhase) {
        self.lock().phase = phase;
    }

    /// A token is saved, as far as the last sign-in, sign-out, or check
    /// knows. `None` until one of them, or the window at start, tells:
    /// without a window nothing reads the keychain just to fill this in.
    pub fn signed_in(&self) -> Option<bool> {
        self.lock().signed_in
    }

    pub fn set_signed_in(&self, signed_in: bool) {
        self.lock().signed_in = Some(signed_in);
    }

    /// Ask the check timer to start the device sign-in.
    pub fn request_connect(&self) {
        self.lock().connect_requested = true;
    }

    /// Ask the check timer to drop the device sign-in. This also clears a
    /// connect request still waiting. The timer handles a cancel first, so
    /// open then cancel starts nothing.
    pub fn request_cancel(&self) {
        let mut inner = self.lock();
        inner.cancel_requested = true;
        inner.connect_requested = false;
    }

    /// Start a new attempt of the sign-in window: end any code still being
    /// polled and ask for a new one. The window keeps the number returned here
    /// and ignores updates from older attempts.
    ///
    /// One lock, paired with [`LivePet::take_flow_requests`], so the timer
    /// never sees the connect without the cancel. It would start a flow, then
    /// drop it, and the window would show a code nobody is polling.
    pub fn request_new_sign_in(&self) -> u64 {
        let mut inner = self.lock();
        inner.attempt = inner.attempt.wrapping_add(1);
        inner.cancel_requested = true;
        inner.connect_requested = true;
        inner.attempt
    }

    /// The waiting cancel and connect requests, taken together.
    pub fn take_flow_requests(&self) -> FlowRequests {
        let mut inner = self.lock();
        let cancel = std::mem::take(&mut inner.cancel_requested);
        let connect = std::mem::take(&mut inner.connect_requested).then_some(inner.attempt);
        FlowRequests { cancel, connect }
    }

    /// When the saved record stops being usable. `None` is no known end. The
    /// tray compares this with the clock, because a token with no refresh
    /// token lapses long before the next check would notice.
    pub fn note_session(&self, until: Option<DateTime<Utc>>) {
        self.lock().session_until = until;
    }

    /// [`LivePet::signed_in`], also counting a record that has since lapsed
    /// as signed out.
    pub fn signed_in_at(&self, now: DateTime<Utc>) -> Option<bool> {
        let inner = self.lock();
        inner
            .signed_in
            .map(|known| known && inner.session_until.is_none_or(|until| until > now))
    }

    pub fn set_sign_in_window_open(&self, open: bool) {
        self.lock().sign_in_window_open = open;
    }

    /// The sign-in window is open or a device sign-in is running.
    pub fn sign_in_active(&self) -> bool {
        let inner = self.lock();
        inner.sign_in_window_open || inner.phase != FlowPhase::Idle
    }

    pub fn set_demo(&self, demo: bool) {
        self.lock().demo = demo;
    }

    pub fn demo(&self) -> bool {
        self.lock().demo
    }

    /// Ask the check timer to post the test message to integration `id`.
    /// A second click before the first is handled does nothing.
    pub fn request_test(&self, id: &str) {
        let mut inner = self.lock();
        if inner.tests.len() < MAX_PENDING_TESTS && !inner.tests.iter().any(|queued| queued == id) {
            inner.tests.push(id.to_string());
        }
    }

    pub fn take_tests(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().tests)
    }

    /// Hand a sign-out to the check timer, with the refresh token to revoke
    /// if the record had one. The caller deletes the keychain copy first.
    pub fn queue_sign_out(&self, refresh_token: Option<&str>) {
        let refresh = refresh_token
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        self.lock().sign_out = Some(SignOutTicket(refresh));
    }

    pub fn take_sign_out(&self) -> Option<SignOutTicket> {
        self.lock().sign_out.take()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }
}

fn finite_scale(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::LivePet;

    #[test]
    fn requests_are_taken_once_and_tests_are_not_queued_twice() {
        let live = LivePet::new(1.0);
        assert_eq!(live.take_flow_requests().connect, None);
        live.request_connect();
        live.request_connect();
        assert!(live.take_flow_requests().connect.is_some());
        assert_eq!(live.take_flow_requests().connect, None);
        live.request_test("ops");
        live.request_test("ops");
        live.request_test("pd");
        assert_eq!(live.take_tests(), ["ops", "pd"]);
        assert!(live.take_tests().is_empty());
        for index in 0..40 {
            live.request_test(&format!("id-{index}"));
        }
        assert_eq!(live.take_tests().len(), super::MAX_PENDING_TESTS);
    }

    #[test]
    fn a_queued_sign_out_is_taken_once_and_hides_its_refresh_token() {
        let live = LivePet::new(1.0);
        assert!(live.take_sign_out().is_none());
        live.queue_sign_out(None);
        assert_eq!(live.take_sign_out().unwrap().refresh_token(), None);
        live.queue_sign_out(Some(""));
        assert_eq!(live.take_sign_out().unwrap().refresh_token(), None);
        live.queue_sign_out(Some("refresh-secret"));
        let ticket = live.take_sign_out().unwrap();
        assert_eq!(ticket.refresh_token(), Some("refresh-secret"));
        assert!(!format!("{ticket:?}").contains("refresh-secret"));
        assert!(live.take_sign_out().is_none());
    }

    #[test]
    fn open_then_cancel_starts_nothing_and_cancel_then_open_starts_one() {
        use super::FlowRequests;
        let live = LivePet::new(1.0);
        let none = FlowRequests {
            cancel: false,
            connect: None,
        };
        assert_eq!(live.take_flow_requests(), none);

        assert_eq!(live.request_new_sign_in(), 1);
        live.request_cancel();
        assert_eq!(
            live.take_flow_requests(),
            FlowRequests {
                cancel: true,
                connect: None
            },
            "the cancel cleared the waiting open"
        );
        assert_eq!(live.take_flow_requests(), none);

        live.request_cancel();
        assert_eq!(live.request_new_sign_in(), 2);
        assert_eq!(
            live.take_flow_requests(),
            FlowRequests {
                cancel: true,
                connect: Some(2)
            },
            "both are taken together, numbered with the newest attempt"
        );
        assert_eq!(live.take_flow_requests(), none);
    }

    #[test]
    fn a_lapsed_record_reads_as_signed_out_without_another_look_at_the_keychain() {
        let live = LivePet::new(1.0);
        let now = chrono::Utc::now();
        assert_eq!(live.signed_in_at(now), None, "not known yet");
        live.set_signed_in(true);
        assert_eq!(live.signed_in_at(now), Some(true), "no known end");
        live.note_session(Some(now + chrono::TimeDelta::seconds(60)));
        assert_eq!(live.signed_in_at(now), Some(true));
        assert_eq!(
            live.signed_in_at(now + chrono::TimeDelta::seconds(60)),
            Some(false)
        );
        assert_eq!(live.signed_in(), Some(true), "the old answer is unchanged");
        live.set_signed_in(false);
        assert_eq!(live.signed_in_at(now), Some(false));
    }

    #[test]
    fn a_sign_in_is_active_while_the_window_is_open_or_a_flow_is_running() {
        use stardust_core::connect::FlowPhase;
        let live = LivePet::new(1.0);
        assert!(!live.sign_in_active());
        live.set_sign_in_window_open(true);
        assert!(live.sign_in_active());
        live.set_sign_in_window_open(false);
        assert!(!live.sign_in_active());
        live.set_phase(FlowPhase::Waiting {
            user_code: "ABCD-EFGH".to_string(),
        });
        assert!(live.sign_in_active());
        live.set_phase(FlowPhase::Idle);
        assert!(!live.sign_in_active());
    }
}
