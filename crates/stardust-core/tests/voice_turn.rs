use stardust_core::voice::{
    enabled, encode_wav, MicError, MicSource, PushToTalk, VoiceClient, VoiceError, VoiceOutcome,
    VoiceState, MAX_RECORDING_SECONDS, MAX_RESPONSE_BYTES, PATH_VOICE_TURN, SAMPLE_RATE,
};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const EXAMPLE: &str = "example-desktop-session";
const ANSWER: &str = r#"{"kind":"answer","transcript":"What changed?","answer":"One alert.","citations":[{"label":"Alert 1","workspace_path":"/findings/1"}]}"#;

struct Request {
    head: String,
    body: Vec<u8>,
}

fn listener() -> (String, TcpListener) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    (
        format!("http://{}", listener.local_addr().unwrap()),
        listener,
    )
}

fn read_request(stream: &TcpStream) -> Request {
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .trim()
        .parse()
        .unwrap();
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Request { head, body }
}

fn serve(reply: impl FnOnce(&mut TcpStream) + Send + 'static) -> (String, JoinHandle<Request>) {
    let (base, listener) = listener();
    let server = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("loopback test did not accept a request: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = read_request(&stream);
        reply(&mut stream);
        request
    });
    (base, server)
}

fn json_reply(status: u16, body: impl Into<String>) -> (String, JoinHandle<Request>) {
    let body = body.into();
    serve(move |stream| {
        write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    })
}

fn client(base: &str) -> VoiceClient {
    VoiceClient::new(base, Some("1")).unwrap()
}

fn wav() -> Vec<u8> {
    encode_wav(&[i16::MIN, 0, i16::MAX]).unwrap()
}

#[test]
fn flag_off_and_invalid_requests_open_no_connection() {
    let (base, listener) = listener();
    for value in [
        None,
        Some(""),
        Some("0"),
        Some("true"),
        Some(" 1"),
        Some("1\n"),
    ] {
        assert!(!enabled(value));
        assert_eq!(
            VoiceClient::new(&base, value)
                .unwrap()
                .turn(EXAMPLE, &wav()),
            Ok(VoiceOutcome::NotAvailable)
        );
    }
    assert!(enabled(Some("1")));
    let client = client(&base);
    let mut oversized = wav();
    oversized.resize(
        44 + 2 * SAMPLE_RATE as usize * MAX_RECORDING_SECONDS as usize + 2,
        0,
    );
    assert_eq!(
        client.turn(EXAMPLE, &oversized),
        Err(VoiceError::RecordingTooLong)
    );
    assert_eq!(
        client.turn(EXAMPLE, b"not a WAV"),
        Err(VoiceError::InvalidAudio)
    );
    assert_eq!(
        client.turn("bad\0example", &wav()),
        Err(VoiceError::Unauthorized)
    );
    assert!(VoiceClient::new("http://example.invalid", Some("1")).is_err());
    listener.set_nonblocking(true).unwrap();
    assert!(matches!(listener.accept(), Err(err) if err.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn sends_one_binary_wav_and_returns_grounded_answer() {
    let (base, server) = json_reply(200, ANSWER);
    let body = wav();
    let VoiceOutcome::Answer(answer) = client(&base).turn(EXAMPLE, &body).unwrap() else {
        panic!("expected an answer");
    };
    assert_eq!(answer.transcript(), "What changed?");
    assert_eq!(answer.answer(), "One alert.");
    assert_eq!(answer.citations()[0].label(), "Alert 1");
    assert_eq!(answer.citations()[0].workspace_path(), "/findings/1");
    let request = server.join().unwrap();
    assert!(request
        .head
        .starts_with(&format!("POST {PATH_VOICE_TURN} HTTP/1.1\r\n")));
    let head = request.head.to_ascii_lowercase();
    assert!(head.contains("\r\ncontent-type: audio/wav\r\n"));
    assert!(head.contains("\r\naccept: application/json\r\n"));
    assert!(head.contains(&format!("\r\nauthorization: bearer {EXAMPLE}\r\n")));
    assert_eq!(request.body, body);
}

#[test]
fn reports_unavailable_for_missing_route_typed_reply_and_refused_connection() {
    for (status, body) in [
        (404, "unrelated HTML"),
        (200, r#"{"kind":"not_available"}"#),
        (503, ""),
    ] {
        let (base, server) = json_reply(status, body);
        assert_eq!(
            client(&base).turn(EXAMPLE, &wav()),
            Ok(VoiceOutcome::NotAvailable)
        );
        server.join().unwrap();
    }
    let (base, listener) = listener();
    drop(listener);
    assert_eq!(
        client(&base).turn(EXAMPLE, &wav()),
        Ok(VoiceOutcome::NotAvailable)
    );
}

#[test]
fn bounds_body_read_time() {
    let (base, server) = serve(|stream| {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\nConnection: close\r\n\r\n{")
            .unwrap();
        // Wait for the timed-out client to close the connection.
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let client = VoiceClient::with_timeout(&base, Some("1"), Duration::from_millis(250)).unwrap();
    assert_eq!(client.turn(EXAMPLE, &wav()), Ok(VoiceOutcome::NotAvailable));
    server.join().unwrap();
}

#[test]
fn rejects_oversized_responses_with_or_without_content_length() {
    let (base, server) = serve(|stream| {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_RESPONSE_BYTES + 1
        )
        .unwrap();
    });
    assert_eq!(
        client(&base).turn(EXAMPLE, &wav()),
        Err(VoiceError::ResponseTooLarge)
    );
    server.join().unwrap();
    let (base, server) = serve(|stream| {
        let body = format!("{ANSWER}{}", " ".repeat(MAX_RESPONSE_BYTES));
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        );
    });
    assert_eq!(
        client(&base).turn(EXAMPLE, &wav()),
        Err(VoiceError::ResponseTooLarge)
    );
    server.join().unwrap();
}

#[test]
fn errors_are_static_and_redirects_are_never_followed() {
    for (status, expected) in [
        (200, VoiceError::BadResponse),
        (401, VoiceError::Unauthorized),
        (400, VoiceError::BadResponse),
    ] {
        let (base, server) = json_reply(status, format!("{{\"reflected\":\"{EXAMPLE}\"}}"));
        let error = client(&base).turn(EXAMPLE, &wav()).unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(EXAMPLE));
        server.join().unwrap();
    }
    let (destination, listener) = listener();
    let (base, server) = serve(move |stream| {
        write!(stream, "HTTP/1.1 302 Redirect\r\nLocation: {destination}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    assert_eq!(
        client(&base).turn(EXAMPLE, &wav()),
        Err(VoiceError::BadResponse)
    );
    server.join().unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(matches!(listener.accept(), Err(err) if err.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn missing_or_unsafe_citations_use_the_fallback() {
    for body in [
        r#"{"kind":"answer","transcript":"Question","answer":"Guess","citations":[]}"#,
        r#"{"kind":"answer","transcript":"Question","answer":"Guess"}"#,
        r#"{"kind":"answer","transcript":"Question","answer":"Guess","citations":[{"label":"Unsafe","workspace_path":"//example.invalid"}]}"#,
    ] {
        let (base, server) = json_reply(200, body);
        let VoiceOutcome::Answer(answer) = client(&base).turn(EXAMPLE, &wav()).unwrap() else {
            panic!("expected a fallback answer");
        };
        assert_eq!(answer.answer(), "I don't know");
        assert!(answer.citations().is_empty());
        server.join().unwrap();
    }
}

struct MemoryMic<'a> {
    samples: &'a [i16],
    active: bool,
}

impl MicSource for MemoryMic<'_> {
    fn start(&mut self) -> Result<u32, MicError> {
        assert!(!self.active);
        self.active = true;
        Ok(SAMPLE_RATE)
    }

    fn read_available(&mut self, output: &mut [i16]) -> Result<usize, MicError> {
        assert!(self.active);
        let count = self.samples.len().min(output.len());
        let (read, remaining) = self.samples.split_at(count);
        output[..count].copy_from_slice(read);
        self.samples = remaining;
        Ok(count)
    }

    fn stop(&mut self) {
        assert!(self.active);
        self.active = false;
    }
}

#[test]
fn press_release_and_worker_completion_move_through_visible_states() {
    let (base, server) = json_reply(200, ANSWER);
    let client = client(&base);
    let mut recorder = PushToTalk::new(
        MemoryMic {
            samples: &[2; 16],
            active: false,
        },
        &client,
    );
    assert_eq!(recorder.state(), VoiceState::Idle);
    recorder.start_recording();
    assert_eq!(recorder.state(), VoiceState::Recording);
    assert!(recorder.poll_recording().is_none());
    let pending = recorder.finish_recording().unwrap();
    assert_eq!(recorder.state(), VoiceState::Sending);
    recorder.start_recording();
    assert_eq!(recorder.state(), VoiceState::Sending);
    recorder.complete(pending.send(&client, EXAMPLE));
    assert!(matches!(recorder.state(), VoiceState::Answer(_)));
    server.join().unwrap();
}

#[test]
fn canceled_turns_cannot_send_or_overwrite_a_newer_turn() {
    let (base, listener) = listener();
    let client = client(&base);
    let max = (SAMPLE_RATE * MAX_RECORDING_SECONDS) as usize;
    let samples = vec![0; max + 9];
    let mut recorder = PushToTalk::new(
        MemoryMic {
            samples: &samples,
            active: false,
        },
        &client,
    );
    recorder.start_recording();
    recorder.cancel();
    assert_eq!(recorder.state(), VoiceState::Idle);
    assert!(recorder.finish_recording().is_none());
    recorder.start_recording();
    let full = recorder
        .poll_recording()
        .expect("full buffer ends the recording");
    assert_eq!(recorder.state(), VoiceState::Sending);
    recorder.cancel();
    recorder.start_recording();
    let next = recorder.finish_recording().unwrap();
    recorder.complete(full.send(&client, EXAMPLE));
    assert_eq!(recorder.state(), VoiceState::Sending);
    recorder.cancel();
    recorder.complete(next.send(&client, EXAMPLE));
    assert_eq!(recorder.state(), VoiceState::Idle);
    listener.set_nonblocking(true).unwrap();
    assert!(matches!(listener.accept(), Err(err) if err.kind() == std::io::ErrorKind::WouldBlock));
}
