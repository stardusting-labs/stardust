use super::{audio, enabled, enabled_from_env, VoiceError};
use crate::api::{parse_server_base, safe_workspace_path};
use serde::Deserialize;
use std::io::{ErrorKind, Read};
use std::time::Duration;

pub const PATH_VOICE_TURN: &str = "/api/pqc/v1/desktop/voice/turn";
pub const MAX_RESPONSE_BYTES: usize = 65_536;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct VoiceCitation {
    label: String,
    workspace_path: String,
}

impl VoiceCitation {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn workspace_path(&self) -> &str {
        &self.workspace_path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoiceAnswer {
    transcript: String,
    answer: String,
    citations: Vec<VoiceCitation>,
}

impl VoiceAnswer {
    pub fn transcript(&self) -> &str {
        &self.transcript
    }

    pub fn answer(&self) -> &str {
        &self.answer
    }

    pub fn citations(&self) -> &[VoiceCitation] {
        &self.citations
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VoiceOutcome {
    Answer(VoiceAnswer),
    NotAvailable,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireResponse {
    Answer {
        transcript: String,
        answer: String,
        #[serde(default)]
        citations: Vec<VoiceCitation>,
    },
    NotAvailable,
}

/// Blocking client for explicit voice turns. Send on a worker when used from a UI.
pub struct VoiceClient {
    enabled: bool,
    url: String,
    agent: ureq::Agent,
}

impl VoiceClient {
    pub fn new(base: &str, flag: Option<&str>) -> Result<Self, VoiceError> {
        Self::with_timeout(base, flag, DEFAULT_TIMEOUT)
    }

    pub fn from_env(base: &str) -> Result<Self, VoiceError> {
        Self::build(base, enabled_from_env(), DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(
        base: &str,
        flag: Option<&str>,
        timeout: Duration,
    ) -> Result<Self, VoiceError> {
        Self::build(base, enabled(flag), timeout)
    }

    fn build(base: &str, enabled: bool, timeout: Duration) -> Result<Self, VoiceError> {
        let base = parse_server_base(base).map_err(|_| VoiceError::ServerBase)?;
        let agent = ureq::AgentBuilder::new()
            .timeout(timeout)
            .timeout_connect(timeout)
            .redirects(0)
            .build();
        Ok(Self {
            enabled,
            url: format!("{base}{PATH_VOICE_TURN}"),
            agent,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Send one canonical WAV body. Disabled turns return before checking audio or credentials.
    pub fn turn(&self, access_token: &str, wav: &[u8]) -> Result<VoiceOutcome, VoiceError> {
        if !self.enabled {
            return Ok(VoiceOutcome::NotAvailable);
        }
        audio::validate_wav(wav)?;
        if access_token.is_empty() || !access_token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(VoiceError::Unauthorized);
        }
        let request = self
            .agent
            .post(&self.url)
            .set("Accept", "application/json")
            .set("Content-Type", "audio/wav")
            .set("Authorization", &format!("Bearer {access_token}"));
        let response = match request.send_bytes(wav) {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(_)) => return Ok(VoiceOutcome::NotAvailable),
        };
        handle_response(response)
    }
}

fn handle_response(response: ureq::Response) -> Result<VoiceOutcome, VoiceError> {
    match response.status() {
        200 => read_response(response),
        404 | 408 | 429 | 500..=599 => Ok(VoiceOutcome::NotAvailable),
        401 | 403 => Err(VoiceError::Unauthorized),
        _ => Err(VoiceError::BadResponse),
    }
}

fn read_response(response: ureq::Response) -> Result<VoiceOutcome, VoiceError> {
    if response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(VoiceError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    match response
        .into_reader()
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut body)
    {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(VoiceOutcome::NotAvailable);
        }
        Err(_) => return Err(VoiceError::BadResponse),
    }
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(VoiceError::ResponseTooLarge);
    }
    decode_response(&body)
}

fn decode_response(body: &[u8]) -> Result<VoiceOutcome, VoiceError> {
    match serde_json::from_slice(body).map_err(|_| VoiceError::BadResponse)? {
        WireResponse::NotAvailable => Ok(VoiceOutcome::NotAvailable),
        WireResponse::Answer {
            transcript,
            answer,
            mut citations,
        } => {
            citations.retain(|citation| {
                !citation.label.trim().is_empty()
                    && safe_workspace_path(&citation.workspace_path)
                        == Some(citation.workspace_path.as_str())
            });
            let answer = if citations.is_empty() || answer.trim().is_empty() {
                citations.clear();
                "I don't know".to_owned()
            } else {
                answer
            };
            Ok(VoiceOutcome::Answer(VoiceAnswer {
                transcript,
                answer,
                citations,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"kind":"answer","transcript":"Question","answer":"One alert","citations":[{"label":"Alert","workspace_path":"/findings/1"}]}"#;

    #[test]
    fn flag_and_invalid_request_checks_run_before_transport() {
        let base = "http://127.0.0.1:1";
        let wav = audio::encode_wav(&[1]).unwrap();
        let off = VoiceClient::new(base, None).unwrap();
        assert_eq!(off.turn("", b""), Ok(VoiceOutcome::NotAvailable));
        let on = VoiceClient::new(base, Some("1")).unwrap();
        assert_eq!(on.turn("", &wav), Err(VoiceError::Unauthorized));
        assert_eq!(on.turn("example", b""), Err(VoiceError::InvalidAudio));
        assert_eq!(
            on.turn("example", &vec![0; audio::MAX_WAV_BYTES + 2]),
            Err(VoiceError::RecordingTooLong)
        );
    }

    #[test]
    fn classifies_statuses_and_ignores_error_body_content() {
        for (status, expected) in [
            (404, Ok(VoiceOutcome::NotAvailable)),
            (429, Ok(VoiceOutcome::NotAvailable)),
            (503, Ok(VoiceOutcome::NotAvailable)),
            (403, Err(VoiceError::Unauthorized)),
            (302, Err(VoiceError::BadResponse)),
            (200, Err(VoiceError::BadResponse)),
        ] {
            let response =
                ureq::Response::new(status, "Example", "reflected example value").unwrap();
            assert_eq!(handle_response(response), expected);
        }
        let unavailable = ureq::Response::new(200, "OK", r#"{"kind":"not_available"}"#).unwrap();
        assert_eq!(handle_response(unavailable), Ok(VoiceOutcome::NotAvailable));
    }

    #[test]
    fn validates_grounded_answers_and_limits_response_size() {
        let padding = " ".repeat(MAX_RESPONSE_BYTES - ANSWER.len());
        let body_at_limit = format!("{ANSWER}{padding}");
        let response = ureq::Response::new(200, "OK", &body_at_limit).unwrap();
        let VoiceOutcome::Answer(answer) = handle_response(response).unwrap() else {
            panic!("expected an answer at the size limit");
        };
        assert_eq!(answer.transcript(), "Question");
        assert_eq!(answer.answer(), "One alert");
        assert_eq!(answer.citations()[0].label(), "Alert");
        assert_eq!(answer.citations()[0].workspace_path(), "/findings/1");

        let over_limit = ureq::Response::new(200, "OK", &format!("{body_at_limit} ")).unwrap();
        assert_eq!(
            handle_response(over_limit),
            Err(VoiceError::ResponseTooLarge)
        );
        let declared: ureq::Response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_RESPONSE_BYTES + 1
        )
        .parse()
        .unwrap();
        assert_eq!(handle_response(declared), Err(VoiceError::ResponseTooLarge));
    }

    #[test]
    fn rejects_unusable_citations_and_falls_back() {
        for body in [
            r#"{"kind":"answer","transcript":"Question","answer":"Guess"}"#.to_owned(),
            ANSWER.replace("/findings/1", "//example.invalid"),
            ANSWER.replace("Alert\"", " \""),
            ANSWER.replace("One alert", " "),
        ] {
            let VoiceOutcome::Answer(answer) = decode_response(body.as_bytes()).unwrap() else {
                panic!("expected a fallback answer");
            };
            assert_eq!(answer.answer(), "I don't know");
            assert!(answer.citations().is_empty());
        }
    }
}
