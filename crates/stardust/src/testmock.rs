//! A loopback stand-in for the ORBIT routes, for tests of the check timer.
//!
//! std only. One request per connection, bound to 127.0.0.1, so no test here
//! can reach a real host. `stardust-core` has its own for its own tests; that
//! one is private to that crate.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// `(path, body)` of each request, in order.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

pub struct MockOrbit {
    base: String,
    seen: Seen,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockOrbit {
    /// `reply` gets the path and the body, and returns the status and the body.
    pub fn start(reply: impl Fn(&str, &str) -> (u16, String) + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback port");
        listener.set_nonblocking(true).expect("nonblocking accept");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let seen = Arc::clone(&seen);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let (path, body) = read_request(&mut stream);
                    let (status, text) = reply(&path, &body);
                    seen.lock().unwrap().push((path, body));
                    let answer = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = stream.write_all(answer.as_bytes());
                }
            })
        };
        Self {
            base,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn count(&self, path: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, _)| seen == path)
            .count()
    }

    pub fn bodies(&self, path: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, _)| seen == path)
            .map(|(_, body)| body.clone())
            .collect()
    }
}

impl Drop for MockOrbit {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut impl Read) -> (String, String) {
    let mut data = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return (String::new(), String::new()),
            Ok(read) => data.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    (path, String::from_utf8_lossy(&body).into_owned())
}
