//! Local stand-in for the ORBIT device sign-in and desktop routes.
//!
//! `scripts/measure-idle-lane.sh` uses it so a signed-in idle run has a
//! server to talk to. Standard library only:
//!
//!     rustc --edition 2021 -O scripts/mock_orbit.rs -o /tmp/mock_orbit
//!     /tmp/mock_orbit 0
//!
//! The first argument is the port on 127.0.0.1. 0, or no argument, picks a
//! free port. The bound address is printed to stdout as
//! `listening 127.0.0.1:PORT`. One request per connection, answered with
//! `Connection: close`. Each request is logged to stderr as
//! `METHOD PATH STATUS`. Headers and bodies are not logged, so the bearer
//! token does not appear in the log.
//!
//! An optional second argument scripts how a device-token poll answers, for
//! trying the sign-in window by hand: `approve` (the default), `pending`,
//! `deny`, `expire`, or `off` (the device routes answer 404). With `pending`
//! the code is never approved, so the window keeps counting down.

#![forbid(unsafe_code)]

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PATH_DEVICE_CODE: &str = "/api/pqc/v1/account/device/code";
const PATH_DEVICE_TOKEN: &str = "/api/pqc/v1/account/device/token";
const PATH_DEVICE_REVOKE: &str = "/api/pqc/v1/account/device/revoke";
const PATH_DESKTOP_STATUS: &str = "/api/pqc/v1/desktop/status";
const PATH_DESKTOP_ALERTS: &str = "/api/pqc/v1/desktop/alerts";

const ACCESS_TOKEN: &str = "mock-access-token";
const TOKEN_LIFETIME_MS: u64 = 300_000;

const DESKTOP_STATUS: &str = r#"{"organization_id":"00000000-0000-4000-8000-000000000001","observation_count":1,"alert_count":1,"high_severity_count":0,"immediate_severity":"high","schedule_owner":"client","workspace_path":"/rust-preview/pqc/studio/app/findings","as_of_unix":1761436800}"#;
const DESKTOP_ALERTS: &str = r#"{"alerts":[{"id":"mock-1","severity":"medium","title":"Example connection finding","created_at":"2026-10-21T00:00:00Z","workspace_path":"/rust-preview/pqc/studio/app/findings/by-severity","immediate":false}]}"#;
const INVALID_GRANT: &str = r#"{"error":"invalid_grant"}"#;
const INVALID_TOKEN: &str = r#"{"error":"invalid_token"}"#;
const AUTHORIZATION_PENDING: &str = r#"{"error":"authorization_pending"}"#;
const ACCESS_DENIED: &str = r#"{"error":"access_denied"}"#;
const EXPIRED_TOKEN: &str = r#"{"error":"expired_token"}"#;

const HEAD_LIMIT: usize = 16 * 1024;
const BODY_LIMIT: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 64;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let port = match args.next() {
        None => 0,
        Some(text) => match text.trim().parse::<u16>() {
            Ok(port) => port,
            Err(_) => return usage(),
        },
    };
    let script = match args.next().as_deref() {
        None | Some("approve") => Script::Approve,
        Some("pending") => Script::Pending,
        Some("deny") => Script::Deny,
        Some("expire") => Script::Expire,
        Some("off") => Script::Off,
        Some(_) => return usage(),
    };
    if args.next().is_some() {
        return usage();
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(err) => {
            say_err(&format!(
                "mock_orbit: cannot listen on 127.0.0.1:{port}: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(err) => {
            say_err(&format!("mock_orbit: cannot read the bound address: {err}"));
            return ExitCode::FAILURE;
        }
    };
    {
        // One write so a reader never sees half of the line.
        let ready = format!("listening {address}\n");
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(ready.as_bytes());
        let _ = stdout.flush();
    }
    serve_forever(&listener, script)
}

fn usage() -> ExitCode {
    say_err("usage: mock_orbit [PORT] [approve|pending|deny|expire|off]   (0 picks a free port)");
    ExitCode::from(2)
}

/// How the device routes answer.
#[derive(Clone, Copy)]
pub enum Script {
    Approve,
    Pending,
    Deny,
    Expire,
    Off,
}

pub fn serve_forever(listener: &TcpListener, script: Script) -> ! {
    let open = Arc::new(AtomicUsize::new(0));
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) => {
                // Out of descriptors or an aborted handshake. Try again shortly.
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            drop(stream);
            continue;
        }
        open.fetch_add(1, Ordering::SeqCst);
        let slot = Slot(Arc::clone(&open));
        // If the thread cannot start, the closure and its slot are dropped.
        let _ = thread::Builder::new()
            .name("mock-orbit-conn".to_string())
            .spawn(move || {
                let _slot = slot;
                serve(stream, script);
            });
    }
}

/// Counts an open connection until the handler thread ends.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Request {
    method: String,
    path: String,
    bearer_ok: bool,
    body: Vec<u8>,
}

enum Incoming {
    /// The peer closed or stalled before sending a byte. Nothing is answered.
    Nothing,
    /// A request that could not be read in full. Answered with `status`.
    Broken {
        method: String,
        path: String,
        status: u16,
    },
    Complete(Request),
}

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        if !self.body.is_empty() {
            head.push_str("Content-Type: application/json\r\nCache-Control: no-store\r\n");
        }
        head.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.body.len()
        ));
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(self.body.as_bytes());
        bytes
    }
}

fn serve(mut stream: TcpStream, script: Script) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let (method, path, reply) = match read_request(&mut stream) {
        Incoming::Nothing => return,
        Incoming::Broken {
            method,
            path,
            status,
        } => (method, path, Reply::empty(status)),
        Incoming::Complete(request) => {
            let reply = route(&request, script);
            (request.method, request.path, reply)
        }
    };
    log_request(&method, &path, reply.status);
    let _ = stream.write_all(&reply.to_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn route(request: &Request, script: Script) -> Reply {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", PATH_DEVICE_CODE | PATH_DEVICE_TOKEN) if matches!(script, Script::Off) => {
            Reply::empty(404)
        }
        ("POST", PATH_DEVICE_CODE) => Reply::json(200, device_code_body()),
        ("POST", PATH_DEVICE_TOKEN) if contains(&request.body, b"refresh_token") => {
            Reply::json(400, INVALID_GRANT)
        }
        ("POST", PATH_DEVICE_TOKEN) => match script {
            Script::Approve | Script::Off => Reply::json(200, token_body()),
            Script::Pending => Reply::json(400, AUTHORIZATION_PENDING),
            Script::Deny => Reply::json(400, ACCESS_DENIED),
            Script::Expire => Reply::json(400, EXPIRED_TOKEN),
        },
        ("GET", PATH_DESKTOP_STATUS | PATH_DESKTOP_ALERTS) if !request.bearer_ok => {
            Reply::json(401, INVALID_TOKEN)
        }
        ("GET", PATH_DESKTOP_STATUS) => Reply::json(200, DESKTOP_STATUS),
        ("GET", PATH_DESKTOP_ALERTS) => Reply::json(200, DESKTOP_ALERTS),
        ("POST", PATH_DEVICE_REVOKE) => Reply::json(200, "{}"),
        _ => Reply::empty(404),
    }
}

fn read_request(stream: &mut TcpStream) -> Incoming {
    let mut data = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    let head_len = loop {
        if let Some(at) = find(&data, b"\r\n\r\n") {
            break at;
        }
        if data.len() > HEAD_LIMIT {
            return broken("-", "-", 431);
        }
        match read_some(stream, &mut chunk, &mut data) {
            Ok(true) => {}
            Ok(false) if data.is_empty() => return Incoming::Nothing,
            Ok(false) => return broken("-", "-", 400),
            Err(_) if data.is_empty() => return Incoming::Nothing,
            Err(status) => return broken("-", "-", status),
        }
    };

    let head = String::from_utf8_lossy(data.get(..head_len).unwrap_or(&[])).into_owned();
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    let path = target.split(['?', '#']).next().unwrap_or("").to_string();
    if method.is_empty()
        || !path.starts_with('/')
        || !version.starts_with("HTTP/1.")
        || parts.next().is_some()
    {
        return broken(&method, &path, 400);
    }

    let expected = format!("Bearer {ACCESS_TOKEN}");
    let mut content_length: Option<usize> = None;
    let mut authorization_headers = 0_u32;
    let mut bearer_ok = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return broken(&method, &path, 400);
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            let Ok(length) = value.parse::<usize>() else {
                return broken(&method, &path, 400);
            };
            if content_length.is_some_and(|seen| seen != length) {
                return broken(&method, &path, 400);
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            // Only Content-Length bodies are read.
            return broken(&method, &path, 400);
        } else if name.eq_ignore_ascii_case("authorization") {
            authorization_headers = authorization_headers.saturating_add(1);
            bearer_ok = value == expected;
        }
    }
    let bearer_ok = bearer_ok && authorization_headers == 1;

    let length = content_length.unwrap_or(0);
    if length > BODY_LIMIT {
        return broken(&method, &path, 413);
    }
    let mut body = data
        .get(head_len.saturating_add(4)..)
        .unwrap_or(&[])
        .to_vec();
    while body.len() < length {
        match read_some(stream, &mut chunk, &mut body) {
            Ok(true) => {}
            Ok(false) => return broken(&method, &path, 400),
            Err(status) => return broken(&method, &path, status),
        }
    }
    body.truncate(length);

    Incoming::Complete(Request {
        method,
        path,
        bearer_ok,
        body,
    })
}

/// Read once into `data`. `Ok(false)` is end of stream. `Err` carries the
/// status to answer with: 408 for a read timeout, 400 otherwise.
fn read_some(stream: &mut TcpStream, chunk: &mut [u8], data: &mut Vec<u8>) -> Result<bool, u16> {
    loop {
        match stream.read(chunk) {
            Ok(0) => return Ok(false),
            Ok(read) => {
                data.extend_from_slice(chunk.get(..read).unwrap_or(&[]));
                return Ok(true);
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(408)
            }
            Err(_) => return Err(400),
        }
    }
}

fn broken(method: &str, path: &str, status: u16) -> Incoming {
    Incoming::Broken {
        method: method.to_string(),
        path: path.to_string(),
        status,
    }
}

fn device_code_body() -> String {
    format!(
        concat!(
            r#"{{"device_code":"{}","user_code":"ABCD-EFGH","#,
            r#""verification_uri":"https://helloorbit.app/rust-preview/pqc/account","#,
            r#""verification_uri_complete":"https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH","#,
            r#""expires_in":600,"interval":1}}"#
        ),
        device_code()
    )
}

fn token_body() -> String {
    let expires_at = now_ms().saturating_add(TOKEN_LIFETIME_MS);
    format!(
        r#"{{"token":"{ACCESS_TOKEN}","access_token":"{ACCESS_TOKEN}","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":{expires_at}}}"#
    )
}

/// 64 lowercase hex characters. Not a secret; it only has to look like one.
fn device_code() -> String {
    let state = RandomState::new();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let mut code = String::with_capacity(64);
    for round in 0_u8..4 {
        let mut hasher = state.build_hasher();
        hasher.write_u8(round);
        hasher.write_u128(nanos);
        code.push_str(&format!("{:016x}", hasher.finish()));
    }
    code
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

fn log_request(method: &str, path: &str, status: u16) {
    say_err(&format!(
        "{} {} {status}",
        printable(method),
        printable(path)
    ));
}

/// Keep log lines to one line of visible ASCII.
fn printable(text: &str) -> String {
    let shown: String = text
        .chars()
        .take(160)
        .map(|ch| if ch.is_ascii_graphic() { ch } else { '?' })
        .collect();
    if shown.is_empty() {
        "-".to_string()
    } else {
        shown
    }
}

fn say_err(line: &str) {
    let mut text = String::with_capacity(line.len() + 1);
    text.push_str(line);
    text.push('\n');
    let _ = io::stderr().lock().write_all(text.as_bytes());
}
