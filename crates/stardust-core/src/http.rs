//! Blocking ORBIT client.
//!
//! Uses the `ureq` agent already in the workspace. The access token is sent
//! as a bearer header and is not written to logs or to the error text.
//! A 404 and a failed connection are ordinary results. The caller retries
//! later. This module does not loop on its own except while waiting for a
//! device-code approval, and that wait sleeps for the server interval.

use crate::api::{
    bumped_interval, AlertsPage, DesktopApi, DesktopStatus, DeviceCodeResponse, DevicePoll,
    DeviceTokenRequest, DeviceTokenResponse, OrbitError, DEVICE_GRANT_TYPE, PATH_DESKTOP_ALERTS,
    PATH_DESKTOP_STATUS, PATH_DEVICE_CODE, PATH_DEVICE_TOKEN,
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
        serde_json::from_slice(&body).map_err(|_| OrbitError::Response)
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
            if token.access_token.is_empty()
                || token
                    .access_token
                    .bytes()
                    .any(|byte| !byte.is_ascii() || byte.is_ascii_whitespace())
            {
                return Err(OrbitError::Response);
            }
            return Ok(DevicePoll::Approved(token));
        }
        match oauth_error(&bytes) {
            Some("authorization_pending") => Ok(DevicePoll::Pending),
            Some("slow_down") => Ok(DevicePoll::SlowDown),
            Some("expired_token") => Err(OrbitError::Expired),
            Some("access_denied") => Err(OrbitError::Denied),
            Some(_) => Err(OrbitError::Refused),
            None if status == 401 => Err(OrbitError::Unauthorized),
            None => Err(classify_transport(status)),
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
        let deadline = Instant::now() + Duration::from_secs(expires_in.clamp(1, 900));
        let mut interval = interval.clamp(1, 30);
        loop {
            if Instant::now() >= deadline {
                return Err(OrbitError::Expired);
            }
            match self.poll_device_token(device_code)? {
                DevicePoll::Approved(token) => return Ok(token),
                DevicePoll::Pending => {}
                DevicePoll::SlowDown => interval = bumped_interval(interval, true),
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
            return Err(classify_desktop(status));
        }
        serde_json::from_slice(&body).map_err(|_| OrbitError::Response)
    }

    fn alerts_now(
        &self,
        access_token: &str,
        cursor: Option<&str>,
    ) -> Result<AlertsPage, OrbitError> {
        let path = match cursor {
            Some(cursor) => format!("{PATH_DESKTOP_ALERTS}?cursor={}", query_encode(cursor)),
            None => PATH_DESKTOP_ALERTS.to_string(),
        };
        let (status, body) = self.desktop("GET", &path, access_token)?;
        if status != 200 {
            return Err(classify_desktop(status));
        }
        serde_json::from_slice(&body).map_err(|_| OrbitError::Response)
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

    fn desktop_alerts<'a>(
        &'a self,
        access_token: &'a str,
        cursor: Option<&'a str>,
    ) -> impl std::future::Future<Output = Result<AlertsPage, OrbitError>> + Send {
        std::future::ready(self.alerts_now(access_token, cursor))
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
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b'-' | b'_'))
}

fn classify_desktop(status: u16) -> OrbitError {
    match status {
        401 => OrbitError::Unauthorized,
        404 => OrbitError::FeatureOff,
        408 | 429 | 500..=599 => OrbitError::Unreachable,
        _ => OrbitError::Response,
    }
}

fn classify_transport(status: u16) -> OrbitError {
    match status {
        408 | 429 | 500..=599 => OrbitError::Unreachable,
        _ => OrbitError::Response,
    }
}

fn classify_sign_in(status: u16, body: &[u8]) -> OrbitError {
    match oauth_error(body) {
        Some("expired_token") => OrbitError::Expired,
        Some("access_denied") => OrbitError::Denied,
        Some(_) => OrbitError::Refused,
        None if status == 401 => OrbitError::Unauthorized,
        None => classify_transport(status),
    }
}

fn oauth_error(body: &[u8]) -> Option<&str> {
    #[derive(Deserialize)]
    struct Body<'a> {
        error: &'a str,
    }
    let Body { error } = serde_json::from_slice(body).ok()?;
    Some(match error {
        "authorization_pending" => "authorization_pending",
        "slow_down" => "slow_down",
        "expired_token" => "expired_token",
        "access_denied" => "access_denied",
        "invalid_request" => "invalid_request",
        "unsupported_grant_type" => "unsupported_grant_type",
        _ => "other",
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

fn query_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::HttpOrbitClient;
    use crate::api::{
        workspace_url, DesktopApi, DevicePoll, OrbitError, DEFAULT_SERVER_BASE,
        PATH_DESKTOP_ALERTS, PATH_DESKTOP_STATUS, PATH_DEVICE_CODE, PATH_DEVICE_TOKEN,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    const EXAMPLE_TOKEN: &str = "example-access";

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        On,
        Off,
        Expired,
    }

    struct Fake {
        polls: u32,
        mode: Mode,
        last_cursor: Option<String>,
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_server_speaks_sign_in_status_alerts_and_the_off_and_expired_states() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(Fake {
            polls: 0,
            mode: Mode::On,
            last_cursor: None,
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
        assert_eq!(started.user_code, "ABCD-EFGH");
        assert!(!format!("{started:?}").contains("abc123"));
        assert!(matches!(
            client.poll_device_token(&started.device_code).unwrap(),
            DevicePoll::Pending
        ));
        let approved = match client.poll_device_token(&started.device_code).unwrap() {
            DevicePoll::Approved(token) => token,
            other => panic!("expected approval, got {other:?}"),
        };
        assert_eq!(approved.access_token, EXAMPLE_TOKEN);
        assert!(!format!("{approved:?}").contains(EXAMPLE_TOKEN));

        let status = client.desktop_status(&approved.access_token).await.unwrap();
        assert_eq!(
            status.counts_by_severity.get(&crate::api::Severity::High),
            Some(&1)
        );
        let page = client
            .desktop_alerts(&approved.access_token, None)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].workspace_path, "/workspace/high");
        assert_eq!(
            workspace_url(DEFAULT_SERVER_BASE, &page.items[0].workspace_path).as_deref(),
            Some("https://helloorbit.app/workspace/high")
        );
        let _ = client
            .desktop_alerts(&approved.access_token, page.next_cursor.as_deref())
            .await
            .unwrap();
        assert_eq!(
            state.lock().unwrap().last_cursor.as_deref(),
            Some("cursor-2")
        );

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
                .desktop_alerts(&approved.access_token, None)
                .await
                .unwrap_err(),
            OrbitError::Unauthorized
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
        let text = String::from_utf8_lossy(&collected);
        let (head, _) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
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
        let (status, reason, body) = route(method, path, query, authorization.as_deref(), state);
        let payload = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(payload.as_bytes())
    }

    fn route(
        method: &str,
        path: &str,
        query: Option<&str>,
        authorization: Option<&str>,
        state: &Mutex<Fake>,
    ) -> (u16, &'static str, String) {
        if method == "POST" && path == PATH_DEVICE_CODE {
            return (
                200,
                "OK",
                r#"{"device_code":"abc123","user_code":"ABCD-EFGH","verification_uri":"https://helloorbit.app/account","verification_uri_complete":"https://helloorbit.app/account?user_code=ABCD-EFGH","expires_in":600,"interval":5}"#
                    .to_string(),
            );
        }
        if method == "POST" && path == PATH_DEVICE_TOKEN {
            let mut fake = state.lock().unwrap_or_else(|err| err.into_inner());
            if fake.polls == 0 {
                fake.polls = 1;
                return (
                    400,
                    "Bad Request",
                    r#"{"error":"authorization_pending"}"#.to_string(),
                );
            }
            return (
                200,
                "OK",
                format!(
                    r#"{{"token":"{EXAMPLE_TOKEN}","access_token":"{EXAMPLE_TOKEN}","token_type":"Bearer","expires_in":900}}"#
                ),
            );
        }
        if method == "GET" && (path == PATH_DESKTOP_STATUS || path == PATH_DESKTOP_ALERTS) {
            let mut fake = state.lock().unwrap_or_else(|err| err.into_inner());
            if path == PATH_DESKTOP_ALERTS {
                fake.last_cursor = query.and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("cursor="))
                        .map(str::to_string)
                });
            }
            let expected = format!("Bearer {EXAMPLE_TOKEN}");
            return match fake.mode {
                Mode::Expired => (
                    401,
                    "Unauthorized",
                    r#"{"error":"invalid_token"}"#.to_string(),
                ),
                Mode::Off => (404, "Not Found", r#"{"error":"not_found"}"#.to_string()),
                Mode::On if authorization != Some(expected.as_str()) => (
                    401,
                    "Unauthorized",
                    r#"{"error":"invalid_token"}"#.to_string(),
                ),
                Mode::On if path == PATH_DESKTOP_STATUS => (
                    200,
                    "OK",
                    r#"{"last_scan_time":"2026-10-21T14:00:00Z","counts_by_severity":{"high":1}}"#
                        .to_string(),
                ),
                Mode::On => (
                    200,
                    "OK",
                    r#"{"items":[{"id":"a1","severity":"high","title":"Example high connection","first_seen":"2026-10-21T14:00:00Z","workspace_path":"/workspace/high"}],"next_cursor":"cursor-2"}"#
                        .to_string(),
                ),
            };
        }
        (404, "Not Found", r#"{"error":"not_found"}"#.to_string())
    }
}
