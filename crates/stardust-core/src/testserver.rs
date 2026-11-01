//! Local HTTP server for unit tests.
//!
//! std only. One request per connection. Each request is recorded and
//! answered by the handler the test passes in.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// Path with the query string, if any.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

pub struct Reply {
    pub status: u16,
    pub body: String,
}

impl Reply {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
        }
    }
}

type Handler = dyn Fn(&Request) -> Reply + Send + Sync;

pub struct TestServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback port");
        let address = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let thread = {
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            thread::spawn(move || serve(&listener, &requests, &stop, handler.as_ref()))
        };
        Self {
            address,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    /// `http://127.0.0.1:PORT`
    pub fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub fn count(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.path() == path)
            .count()
    }
}

/// Sends sink posts to a [`TestServer`] over plain loopback HTTP. The path
/// and query of the real destination are kept, so a test can see which
/// destination a post was meant for.
pub struct LocalTransport {
    base: String,
    agent: ureq::Agent,
}

impl LocalTransport {
    pub fn to(server: &TestServer) -> Self {
        Self {
            base: server.base(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(2))
                .redirects(0)
                .build(),
        }
    }

    /// A transport whose every post fails to connect.
    pub fn closed() -> Self {
        Self {
            base: "http://127.0.0.1:1".to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_millis(500))
                .build(),
        }
    }
}

impl crate::sink::Transport for LocalTransport {
    fn post_json(
        &self,
        url: &crate::outbound::CheckedUrl,
        body: &[u8],
    ) -> Result<u16, crate::sink::SinkError> {
        let rest = url.url.strip_prefix("https://").unwrap_or(&url.url);
        let path = rest.find('/').map(|at| &rest[at..]).unwrap_or("/");
        let target = format!("{}{path}", self.base);
        match self
            .agent
            .post(&target)
            .set("Content-Type", "application/json")
            .send_bytes(body)
        {
            Ok(response) => Ok(response.status()),
            Err(ureq::Error::Status(status, _)) => Ok(status),
            Err(_) => Err(crate::sink::SinkError::Unreachable),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    listener: &TcpListener,
    requests: &Mutex<Vec<Request>>,
    stop: &AtomicBool,
    handler: &Handler,
) {
    listener.set_nonblocking(true).ok();
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                if let Some(request) = read_request(&mut stream) {
                    let reply = handler(&request);
                    requests
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .push(request);
                    let _ = write_reply(&mut stream, &reply);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut collected = Vec::new();
    let mut buf = [0_u8; 2048];
    let head_end = loop {
        if let Some(at) = collected
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            break at + 4;
        }
        if collected.len() >= 16_384 {
            return None;
        }
        let read = stream.read(&mut buf).ok()?;
        if read == 0 {
            return None;
        }
        collected.extend_from_slice(&buf[..read]);
    };
    let head = String::from_utf8_lossy(&collected[..head_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let wanted = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0)
        .min(65_536);
    while collected.len() < head_end + wanted {
        let read = stream.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        collected.extend_from_slice(&buf[..read]);
    }
    let end = collected.len().min(head_end + wanted);
    let body = String::from_utf8_lossy(&collected[head_end..end]).to_string();
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

fn write_reply(stream: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
    let content_type = if reply.body.is_empty() {
        ""
    } else {
        "Content-Type: application/json\r\n"
    };
    let payload = format!(
        "HTTP/1.1 {} Test\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    stream.write_all(payload.as_bytes())
}
