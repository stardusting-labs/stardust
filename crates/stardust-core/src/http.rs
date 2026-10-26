//! Blocking ORBIT client.
//!
//! Uses the `ureq` agent already in the workspace. The access token is sent
//! as a bearer header and is not written to logs or to the error text.
//! A 404 and a failed connection are ordinary results. The caller retries
//! later. This module does not loop on its own except while waiting for a
//! device-code approval, and that wait sleeps for the server interval.

use crate::api::{
    bumped_interval, AlertsPage, DesktopApi, DesktopStatus, DeviceCodeResponse, DevicePoll,
    DeviceTokenRequest, DeviceTokenResponse, OrbitError, RefreshTokenRequest,
    DEVICE_CODE_TTL_SECONDS, DEVICE_GRANT_TYPE, PATH_DESKTOP_ALERTS, PATH_DESKTOP_STATUS,
    PATH_DEVICE_CODE, PATH_DEVICE_TOKEN, PATH_TOKEN_REFRESH, REFRESH_GRANT_TYPE,
};
use serde::Deserialize;
use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};

const BODY_LIMIT: u64 = 65_536;

pub struct HttpOrbitClient {
    base: String,
    agent: ureq::Agent,
}

impl HttpOrbitClient {
    pub fn new(base: &str) -> Result<Self, OrbitError> {
        Self::with_timeout(base, Duration::from_secs(10))
    }

    pub fn with_timeout(base: &str, timeout: Duration) -> Result<Self, OrbitError> {
        let base = crate::api::parse_server_base(base)?;
        let agent = ureq::AgentBuilder::new()
            .timeout(timeout)
            .timeout_connect(timeout)
            .redirects(0)
            .build();
        Ok(Self { base, agent })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn start_device_code(&self) -> Result<DeviceCodeResponse, OrbitError> {
        let (status, body) = self.exchange("POST", PATH_DEVICE_CODE, None, Some(b"{}"))?;
        if status != 200 {
            return Err(classify_sign_in(status, &body));
        }
        let started: DeviceCodeResponse =
            serde_json::from_slice(&body).map_err(|_| OrbitError::Response)?;
        if !valid_device_code(&started.device_code) || !valid_user_code(&started.user_code) {
            return Err(OrbitError::Response);
        }
        Ok(started)
    }

    pub fn poll_device_token(&self, device_code: &str) -> Result<DevicePoll, OrbitError> {
        if !valid_device_code(device_code) {
            return Err(OrbitError::Refused);
        }
        let request = DeviceTokenRequest {
            grant_type: DEVICE_GRANT_TYPE.to_string(),
            device_code: device_code.to_string(),
        };
        let body = serde_json::to_vec(&request).map_err(|_| OrbitError::Refused)?;
        let (status, bytes) = self.exchange("POST", PATH_DEVICE_TOKEN, None, Some(&body))?;
        if status == 200 {
            let token: DeviceTokenResponse =
                serde_json::from_slice(&bytes).map_err(|_| OrbitError::Response)?;
            if !accepted_token(&token) {
                return Err(OrbitError::Response);
            }
            return Ok(DevicePoll::Approved(token));
        }
        match oauth_error(&bytes) {
            Some(err) if err.code == "authorization_pending" => Ok(DevicePoll::Pending),
            Some(err) if err.code == "slow_down" => Ok(DevicePoll::SlowDown {
                interval: err
                    .interval
                    .unwrap_or_else(|| bumped_interval(5, true))
                    .clamp(1, 30),
            }),
            Some(err) => Err(classify_code(status, err.code)),
            None => Err(classify_status(status)),
        }
    }

    /// Trade a refresh token for a new access token.
    ///
    /// A rejected refresh token (`invalid_grant`, or a 401) is
    /// [`OrbitError::Unauthorized`], and the caller signs the user out.
    pub fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<DeviceTokenResponse, OrbitError> {
        if !token_chars(refresh_token) {
            return Err(OrbitError::Unauthorized);
        }
        let request = RefreshTokenRequest {
            grant_type: REFRESH_GRANT_TYPE.to_string(),
            refresh_token: refresh_token.to_string(),
        };
        let body = serde_json::to_vec(&request).map_err(|_| OrbitError::Refused)?;
        let (status, bytes) = self.exchange("POST", PATH_TOKEN_REFRESH, None, Some(&body))?;
        if status == 200 {
            let token: DeviceTokenResponse =
                serde_json::from_slice(&bytes).map_err(|_| OrbitError::Response)?;
            if !accepted_token(&token) {
                return Err(OrbitError::Response);
            }
            return Ok(token);
        }
        match oauth_error(&bytes) {
            Some(err) => Err(classify_code(status, err.code)),
            None => Err(classify_status(status)),
        }
    }

    /// Poll until the browser approves the code, the code expires, or the server refuses.
    ///
    /// Sleeps for the server interval. A `slow_down` response lengthens the wait.
    pub fn wait_for_device_token(
        &self,
        device_code: &str,
        expires_in: u64,
        interval: u64,
    ) -> Result<DeviceTokenResponse, OrbitError> {
        let deadline =
            Instant::now() + Duration::from_secs(expires_in.clamp(1, DEVICE_CODE_TTL_SECONDS));
        let mut interval = interval.clamp(1, 30);
        loop {
            if Instant::now() >= deadline {
                return Err(OrbitError::Expired);
            }
            match self.poll_device_token(device_code)? {
                DevicePoll::Approved(token) => return Ok(token),
                DevicePoll::Pending => {}
                DevicePoll::SlowDown { interval: next } => interval = next.clamp(1, 30),
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let pause = Duration::from_secs(interval).min(remaining);
            if pause.is_zero() {
                return Err(OrbitError::Expired);
            }
            thread::sleep(pause);
        }
    }

    fn status_now(&self, access_token: &str) -> Result<DesktopStatus, OrbitError> {
        let (status, body) = self.desktop("GET", PATH_DESKTOP_STATUS, access_token)?;
        if status != 200 {
            return Err(classify_desktop(status, &body));
        }
        let parsed: DesktopStatus =
            serde_json::from_slice(&body).map_err(|_| OrbitError::Response)?;
        if !accepted_status(&parsed) {
            return Err(OrbitError::Response);
        }
        Ok(parsed)
    }

    fn alerts_now(&self, access_token: &str) -> Result<AlertsPage, OrbitError> {
        let (status, body) = self.desktop("GET", PATH_DESKTOP_ALERTS, access_token)?;
        if status != 200 {
            return Err(classify_desktop(status, &body));
        }
        let parsed: AlertsPage = serde_json::from_slice(&body).map_err(|_| OrbitError::Response)?;
        if !accepted_alerts(&parsed) {
            return Err(OrbitError::Response);
        }
        Ok(parsed)
    }

    fn desktop(
        &self,
        method: &str,
        path: &str,
        access_token: &str,
    ) -> Result<(u16, Vec<u8>), OrbitError> {
        let header = bearer(access_token)?;
        self.exchange(method, path, Some(&header), None)
    }

    fn exchange(
        &self,
        method: &str,
        path: &str,
        authorization: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), OrbitError> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let request = match method {
            "GET" => self.agent.get(&url),
            "POST" => self.agent.post(&url),
            _ => return Err(OrbitError::Refused),
        };
        let request = request.set("Accept", "application/json");
        let request = if body.is_some() {
            request.set("Content-Type", "application/json")
        } else {
            request
        };
        let request = if let Some(authorization) = authorization {
            request.set("Authorization", authorization)
        } else {
            request
        };
        let result = if let Some(body) = body {
            request.send_bytes(body)
        } else {
            request.call()
        };
        match result {
            Ok(response) => read_limited(response.status(), response),
            Err(ureq::Error::Status(status, response)) => read_limited(status, response),
            Err(_) => Err(OrbitError::Unreachable),
        }
    }
}

impl DesktopApi for HttpOrbitClient {
    fn desktop_status(
        &self,
        access_token: &str,
    ) -> impl std::future::Future<Output = Result<DesktopStatus, OrbitError>> + Send {
        std::future::ready(self.status_now(access_token))
    }

    fn desktop_alerts(
        &self,
        access_token: &str,
    ) -> impl std::future::Future<Output = Result<AlertsPage, OrbitError>> + Send {
        std::future::ready(self.alerts_now(access_token))
    }
}

fn bearer(access_token: &str) -> Result<String, OrbitError> {
    if access_token.is_empty()
        || access_token
            .bytes()
            .any(|byte| !byte.is_ascii() || byte.is_ascii_whitespace())
    {
        return Err(OrbitError::Unauthorized);
    }
    Ok(format!("Bearer {access_token}"))
}

fn valid_device_code(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_user_code(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(left), Some(right)) = (parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    left.len() == 4
        && right.len() == 4
        && left
            .bytes()
            .chain(right.bytes())
            .all(|byte| ALPHABET.contains(&byte))
}

fn token_chars(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii() && !byte.is_ascii_whitespace())
}

fn accepted_token(token: &DeviceTokenResponse) -> bool {
    token_chars(&token.access_token)
        && token.token == token.access_token
        && token.token_type == "Bearer"
        && token.expires_in > 0
        && token.expires_at_epoch_ms > 0
        && token.refresh_token.as_deref().is_none_or(token_chars)
        && token.refresh_expires_in != Some(0)
}

fn accepted_status(status: &DesktopStatus) -> bool {
    !status.organization_id.is_empty()
        && status.organization_id.len() <= 80
        && status
            .organization_id
            .bytes()
            .all(|byte| byte.is_ascii() && !byte.is_ascii_whitespace())
        && status.as_of_unix >= 0
        && crate::api::safe_workspace_path(&status.workspace_path).is_some()
}

fn accepted_alerts(page: &AlertsPage) -> bool {
    page.alerts.iter().all(|alert| {
        (1..=80).contains(&alert.id.len())
            && (1..=120).contains(&alert.title.len())
            && alert.immediate == (alert.severity == crate::api::Severity::High)
            && crate::api::safe_workspace_path(&alert.workspace_path).is_some()
    })
}

fn classify_desktop(status: u16, body: &[u8]) -> OrbitError {
    match oauth_error(body) {
        Some(err) => classify_code(status, err.code),
        None => classify_status(status),
    }
}

fn classify_sign_in(status: u16, body: &[u8]) -> OrbitError {
    match oauth_error(body) {
        Some(err) => classify_code(status, err.code),
        None => classify_status(status),
    }
}

fn classify_code(status: u16, code: &str) -> OrbitError {
    match code {
        "expired_token" => OrbitError::Expired,
        "access_denied" | "origin_denied" => OrbitError::Denied,
        "invalid_grant" => OrbitError::Unauthorized,
        "invalid_token" | "invalid_request" if status == 401 => OrbitError::Unauthorized,
        "temporarily_unavailable" => OrbitError::Unreachable,
        _ => classify_status(status),
    }
}

fn classify_status(status: u16) -> OrbitError {
    match status {
        401 => OrbitError::Unauthorized,
        403 => OrbitError::Denied,
        404 => OrbitError::FeatureOff,
        408 | 429 | 500..=599 => OrbitError::Unreachable,
        _ => OrbitError::Response,
    }
}

struct OauthError {
    code: &'static str,
    interval: Option<u64>,
}

fn oauth_error(body: &[u8]) -> Option<OauthError> {
    #[derive(Deserialize)]
    struct Body<'a> {
        error: &'a str,
        #[serde(default)]
        interval: Option<u64>,
    }
    let parsed: Body<'_> = serde_json::from_slice(body).ok()?;
    let code = match parsed.error {
        "authorization_pending" => "authorization_pending",
        "slow_down" => "slow_down",
        "expired_token" => "expired_token",
        "access_denied" => "access_denied",
        "origin_denied" => "origin_denied",
        "invalid_request" => "invalid_request",
        "invalid_token" => "invalid_token",
        "invalid_grant" => "invalid_grant",
        "unsupported_grant_type" => "unsupported_grant_type",
        "temporarily_unavailable" => "temporarily_unavailable",
        _ => "other",
    };
    Some(OauthError {
        code,
        interval: parsed.interval,
    })
}

fn read_limited(status: u16, response: ureq::Response) -> Result<(u16, Vec<u8>), OrbitError> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(BODY_LIMIT)
        .read_to_end(&mut body)
        .map_err(|_| OrbitError::Unreachable)?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::HttpOrbitClient;
    use crate::api::{
        workspace_url, DesktopApi, DevicePoll, ImmediateSeverity, OrbitError, ScheduleOwner,
        Severity, DEFAULT_SERVER_BASE, PATH_DESKTOP_ALERTS, PATH_DESKTOP_STATUS, PATH_DEVICE_CODE,
        PATH_DEVICE_TOKEN, WORKSPACE_FINDINGS, WORKSPACE_NEEDS_ATTENTION,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    const EXAMPLE_TOKEN: &str = "example-access";
    const DEVICE_CODE: &str = "abababababababababababababababababababababababababababababababab";

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        On,
        Off,
        Expired,
        Busy,
    }

    struct Fake {
        polls: u32,
        mode: Mode,
        saw_query: bool,
        refreshes: u32,
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_server_speaks_sign_in_status_alerts_and_the_off_and_expired_states() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(Fake {
            polls: 0,
            mode: Mode::On,
            saw_query: false,
            refreshes: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            thread::spawn(move || serve(listener, state, stop))
        };
        let client =
            HttpOrbitClient::with_timeout(&format!("http://{address}"), Duration::from_secs(2))
                .unwrap();

        let started = client.start_device_code().unwrap();
        assert_eq!(started.device_code, DEVICE_CODE);
        assert_eq!(started.user_code, "ABCD-EFGH");
        assert_eq!(started.expires_in, 600);
        assert_eq!(started.interval, 5);
        assert_eq!(
            started.verification_uri,
            "https://helloorbit.app/rust-preview/pqc/account"
        );
        assert!(!format!("{started:?}").contains(DEVICE_CODE));
        assert!(matches!(
            client.poll_device_token(&started.device_code).unwrap(),
            DevicePoll::Pending
        ));
        assert!(matches!(
            client.poll_device_token(&started.device_code).unwrap(),
            DevicePoll::SlowDown { interval: 10 }
        ));
        let approved = match client.poll_device_token(&started.device_code).unwrap() {
            DevicePoll::Approved(token) => token,
            other => panic!("expected approval, got {other:?}"),
        };
        assert_eq!(approved.token, EXAMPLE_TOKEN);
        assert_eq!(approved.access_token, EXAMPLE_TOKEN);
        assert_eq!(approved.token_type, "Bearer");
        assert_eq!(approved.expires_in, 300);
        assert_eq!(approved.expires_at_epoch_ms, 1_893_456_000_000);
        assert!(!format!("{approved:?}").contains(EXAMPLE_TOKEN));

        let status = client.desktop_status(&approved.access_token).await.unwrap();
        assert_eq!(
            status.organization_id,
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(status.observation_count, 1);
        assert_eq!(status.alert_count, 1);
        assert_eq!(status.high_severity_count, 1);
        assert_eq!(status.immediate_severity, ImmediateSeverity::High);
        assert_eq!(status.schedule_owner, ScheduleOwner::Client);
        assert_eq!(status.workspace_path, WORKSPACE_FINDINGS);
        let page = client.desktop_alerts(&approved.access_token).await.unwrap();
        assert_eq!(page.alerts.len(), 1);
        assert_eq!(page.alerts[0].severity, Severity::High);
        assert!(page.alerts[0].immediate);
        assert_eq!(page.alerts[0].workspace_path, WORKSPACE_NEEDS_ATTENTION);
        assert_eq!(
            workspace_url(DEFAULT_SERVER_BASE, &page.alerts[0].workspace_path).as_deref(),
            Some("https://helloorbit.app/rust-preview/pqc/studio/app/control-room/needs-attention")
        );
        assert!(!state.lock().unwrap().saw_query);

        state.lock().unwrap().mode = Mode::Off;
        assert_eq!(
            client
                .desktop_status(&approved.access_token)
                .await
                .unwrap_err(),
            OrbitError::FeatureOff
        );
        state.lock().unwrap().mode = Mode::Expired;
        assert_eq!(
            client
                .desktop_alerts(&approved.access_token)
                .await
                .unwrap_err(),
            OrbitError::Unauthorized
        );
        state.lock().unwrap().mode = Mode::Busy;
        assert_eq!(
            client
                .desktop_status(&approved.access_token)
                .await
                .unwrap_err(),
            OrbitError::Unreachable
        );

        let closed =
            HttpOrbitClient::with_timeout("http://127.0.0.1:1", Duration::from_secs(1)).unwrap();
        assert_eq!(
            closed.desktop_status(EXAMPLE_TOKEN).await.unwrap_err(),
            OrbitError::Unreachable
        );
        assert_eq!(
            closed.desktop_status("").await.unwrap_err(),
            OrbitError::Unauthorized
        );

        stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(address);
        let _ = server.join();
    }

    #[test]
    fn local_server_trades_a_refresh_token_and_refuses_an_unknown_one() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(Fake {
            polls: 0,
            mode: Mode::On,
            saw_query: false,
            refreshes: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            thread::spawn(move || serve(listener, state, stop))
        };
        let client =
            HttpOrbitClient::with_timeout(&format!("http://{address}"), Duration::from_secs(2))
                .unwrap();
        let next = client.refresh_access_token("example-refresh").unwrap();
        assert_eq!(next.access_token, EXAMPLE_TOKEN);
        assert_eq!(next.refresh_token.as_deref(), Some("example-refresh-next"));
        assert_eq!(next.refresh_expires_in, Some(86_400));
        assert!(!format!("{next:?}").contains("example-refresh-next"));
        assert_eq!(
            client.refresh_access_token("revoked-refresh").unwrap_err(),
            OrbitError::Unauthorized
        );
        assert_eq!(
            client.refresh_access_token("has space").unwrap_err(),
            OrbitError::Unauthorized
        );
        assert_eq!(state.lock().unwrap().refreshes, 2);
        stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(address);
        let _ = server.join();
    }

    fn serve(listener: TcpListener, state: Arc<Mutex<Fake>>, stop: Arc<AtomicBool>) {
        listener.set_nonblocking(true).ok();
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = handle(&mut stream, &state);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }

    fn handle(stream: &mut std::net::TcpStream, state: &Mutex<Fake>) -> std::io::Result<()> {
        let mut collected = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            if collected.windows(4).any(|window| window == b"\r\n\r\n") || collected.len() >= 8192 {
                break;
            }
            let read = stream.read(&mut buf)?;
            if read == 0 {
                break;
            }
            collected.extend_from_slice(&buf[..read]);
        }
        let head_end = collected
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
            .unwrap_or(collected.len());
        let wanted = String::from_utf8_lossy(&collected[..head_end])
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0)
            .min(8192);
        while collected.len() < head_end + wanted {
            let read = stream.read(&mut buf)?;
            if read == 0 {
                break;
            }
            collected.extend_from_slice(&buf[..read]);
        }
        let text = String::from_utf8_lossy(&collected);
        let (head, request_body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
        let mut lines = head.lines();
        let request = lines.next().unwrap_or("");
        let mut parts = request.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        let (path, query) = target
            .split_once('?')
            .map(|(path, query)| (path, Some(query)))
            .unwrap_or((target, None));
        let authorization = lines.find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_string())
        });
        let (status, reason, body) = route(
            method,
            path,
            query,
            authorization.as_deref(),
            request_body,
            state,
        );
        let content_type = if body.is_empty() {
            ""
        } else {
            "Content-Type: application/json\r\n"
        };
        let payload = format!(
            "HTTP/1.1 {status} {reason}\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(payload.as_bytes())
    }

    fn route(
        method: &str,
        path: &str,
        query: Option<&str>,
        authorization: Option<&str>,
        request_body: &str,
        state: &Mutex<Fake>,
    ) -> (u16, &'static str, String) {
        if query.is_some() {
            state
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .saw_query = true;
            return (
                400,
                "Bad Request",
                r#"{"error":"invalid_request"}"#.to_string(),
            );
        }
        if method == "POST" && path == PATH_DEVICE_CODE {
            return (
                200,
                "OK",
                format!(
                    r#"{{"device_code":"{DEVICE_CODE}","user_code":"ABCD-EFGH","verification_uri":"https://helloorbit.app/rust-preview/pqc/account","verification_uri_complete":"https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH","expires_in":600,"interval":5}}"#
                ),
            );
        }
        if method == "POST"
            && path == PATH_DEVICE_TOKEN
            && request_body.contains(r#""grant_type":"refresh_token""#)
        {
            let mut fake = state.lock().unwrap_or_else(|err| err.into_inner());
            fake.refreshes = fake.refreshes.saturating_add(1);
            return if request_body.contains(r#""refresh_token":"example-refresh""#) {
                (
                    200,
                    "OK",
                    format!(
                        r#"{{"token":"{EXAMPLE_TOKEN}","access_token":"{EXAMPLE_TOKEN}","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":1893456000000,"refresh_token":"example-refresh-next","refresh_expires_in":86400}}"#
                    ),
                )
            } else {
                (
                    400,
                    "Bad Request",
                    r#"{"error":"invalid_grant"}"#.to_string(),
                )
            };
        }
        if method == "POST" && path == PATH_DEVICE_TOKEN {
            let mut fake = state.lock().unwrap_or_else(|err| err.into_inner());
            let polls = fake.polls;
            fake.polls = fake.polls.saturating_add(1);
            return match polls {
                0 => (
                    400,
                    "Bad Request",
                    r#"{"error":"authorization_pending"}"#.to_string(),
                ),
                1 => (
                    400,
                    "Bad Request",
                    r#"{"error":"slow_down","interval":10}"#.to_string(),
                ),
                _ => (
                    200,
                    "OK",
                    format!(
                        r#"{{"token":"{EXAMPLE_TOKEN}","access_token":"{EXAMPLE_TOKEN}","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":1893456000000}}"#
                    ),
                ),
            };
        }
        if method == "GET" && (path == PATH_DESKTOP_STATUS || path == PATH_DESKTOP_ALERTS) {
            let fake = state.lock().unwrap_or_else(|err| err.into_inner());
            let expected = format!("Bearer {EXAMPLE_TOKEN}");
            return match fake.mode {
                Mode::Expired => (
                    401,
                    "Unauthorized",
                    r#"{"error":"invalid_token"}"#.to_string(),
                ),
                Mode::Off => (404, "Not Found", String::new()),
                Mode::Busy => (
                    503,
                    "Service Unavailable",
                    r#"{"error":"temporarily_unavailable"}"#.to_string(),
                ),
                Mode::On if authorization != Some(expected.as_str()) => (
                    401,
                    "Unauthorized",
                    r#"{"error":"invalid_token"}"#.to_string(),
                ),
                Mode::On if path == PATH_DESKTOP_STATUS => (
                    200,
                    "OK",
                    r#"{"organization_id":"00000000-0000-4000-8000-000000000001","observation_count":1,"alert_count":1,"high_severity_count":1,"immediate_severity":"high","schedule_owner":"client","workspace_path":"/rust-preview/pqc/studio/app/findings","as_of_unix":1761436800}"#
                        .to_string(),
                ),
                Mode::On => (
                    200,
                    "OK",
                    r#"{"alerts":[{"id":"mp-41-key-exchange-rule-id","severity":"high","title":"api.example:443 key-exchange x25519","created_at":"2026-10-26T00:00:00Z","workspace_path":"/rust-preview/pqc/studio/app/control-room/needs-attention","immediate":true}]}"#
                        .to_string(),
                ),
            };
        }
        (404, "Not Found", String::new())
    }
}
