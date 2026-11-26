use super::audio::sample_limit;
use super::{
    encode_wav, resample_to_16khz, VoiceAnswer, VoiceClient, VoiceError, VoiceOutcome,
    MAX_RECORDING_SECONDS,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const MAX_DURATION: Duration = Duration::from_secs(MAX_RECORDING_SECONDS as u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MicError;

/// An explicit capture session that produces signed, mono PCM16 samples.
/// Implementations may buffer at most one turn and must stop capturing by its time limit.
pub trait MicSource {
    /// Start a fresh capture and return its sample rate in Hz. On error the source is stopped.
    fn start(&mut self) -> Result<u32, MicError>;

    /// Drain immediately available frames into `output`, returning the number written.
    /// This method does not wait for new audio and cannot exceed `output.len()`.
    fn read_available(&mut self, output: &mut [i16]) -> Result<usize, MicError>;

    /// Stop capture and discard frames that have not been read.
    fn stop(&mut self);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceState<'a> {
    Idle,
    Recording,
    Sending,
    Answer(&'a VoiceAnswer),
    NotAvailable,
    Error(VoiceError),
}

struct Recording {
    started: Instant,
    rate: u32,
    samples: Vec<i16>,
    written: usize,
}

enum State {
    Idle,
    Recording(Recording),
    Sending(Arc<AtomicBool>),
    Answer(VoiceAnswer),
    NotAvailable,
    Error(VoiceError),
}

/// Audio for one turn. Move it to a worker to send without blocking the UI.
#[must_use]
pub struct PendingTurn {
    wav: Vec<u8>,
    canceled: Arc<AtomicBool>,
}

impl PendingTurn {
    /// A canceled queued turn opens no connection. A subsequent cancellation ignores the result.
    pub fn send(self, client: &VoiceClient, access_token: &str) -> CompletedTurn {
        let outcome = if self.canceled.load(Ordering::Acquire) {
            Ok(VoiceOutcome::NotAvailable)
        } else {
            client.turn(access_token, &self.wav)
        };
        CompletedTurn {
            canceled: self.canceled,
            outcome,
        }
    }
}

/// Worker result to return to the state machine that issued the turn.
#[must_use]
pub struct CompletedTurn {
    canceled: Arc<AtomicBool>,
    outcome: Result<VoiceOutcome, VoiceError>,
}

/// The in-memory state machine for an explicitly initiated turn.
pub struct PushToTalk<M: MicSource> {
    mic: M,
    enabled: bool,
    state: State,
}

impl<M: MicSource> PushToTalk<M> {
    /// Share the cloud client's feature gate with the microphone controller.
    pub fn new(mic: M, client: &VoiceClient) -> Self {
        Self {
            mic,
            enabled: client.is_enabled(),
            state: State::Idle,
        }
    }

    pub fn state(&self) -> VoiceState<'_> {
        match &self.state {
            State::Idle => VoiceState::Idle,
            State::Recording(_) => VoiceState::Recording,
            State::Sending(_) => VoiceState::Sending,
            State::Answer(answer) => VoiceState::Answer(answer),
            State::NotAvailable => VoiceState::NotAvailable,
            State::Error(error) => VoiceState::Error(*error),
        }
    }

    /// Start on an explicit press. Repeated presses during recording or sending are ignored.
    pub fn start_recording(&mut self) {
        if !self.enabled || matches!(self.state, State::Recording(_) | State::Sending(_)) {
            return;
        }
        let started = Instant::now();
        let Ok(rate) = self.mic.start() else {
            self.state = State::Error(VoiceError::Microphone);
            return;
        };
        self.state = match sample_limit(rate) {
            Ok(limit) => State::Recording(Recording {
                started,
                rate,
                samples: vec![0; limit],
                written: 0,
            }),
            Err(error) => {
                self.mic.stop();
                State::Error(error)
            }
        };
    }

    /// Drain audio while the button is held. A full or expired recording produces a turn.
    pub fn poll_recording(&mut self) -> Option<PendingTurn> {
        self.advance_recording(false)
    }

    /// Release the button and queue the captured turn, if any.
    pub fn finish_recording(&mut self) -> Option<PendingTurn> {
        self.advance_recording(true)
    }

    fn advance_recording(&mut self, released: bool) -> Option<PendingTurn> {
        let State::Recording(recording) = &mut self.state else {
            return None;
        };
        let remaining = &mut recording.samples[recording.written..];
        match self.mic.read_available(remaining) {
            Ok(read) if read <= remaining.len() => recording.written += read,
            Ok(_) | Err(_) => {
                self.mic.stop();
                self.state = State::Error(VoiceError::Microphone);
                return None;
            }
        }
        if !released
            && recording.written < recording.samples.len()
            && recording.started.elapsed() < MAX_DURATION
        {
            return None;
        }
        self.mic.stop();
        let audio = &recording.samples[..recording.written];
        let wav = match resample_to_16khz(audio, recording.rate)
            .and_then(|samples| encode_wav(&samples))
        {
            Ok(wav) => wav,
            Err(error) => {
                self.state = State::Error(error);
                return None;
            }
        };
        let canceled = Arc::new(AtomicBool::new(false));
        self.state = State::Sending(Arc::clone(&canceled));
        Some(PendingTurn { wav, canceled })
    }

    /// Discard an active recording or invalidate an outstanding worker result.
    pub fn cancel(&mut self) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Recording(_) => self.mic.stop(),
            State::Sending(canceled) => canceled.store(true, Ordering::Release),
            State::Idle | State::Answer(_) | State::NotAvailable | State::Error(_) => {}
        }
    }

    /// Apply only the result of the currently outstanding turn.
    pub fn complete(&mut self, completed: CompletedTurn) {
        if !matches!(&self.state, State::Sending(canceled) if Arc::ptr_eq(canceled, &completed.canceled))
        {
            return;
        }
        self.state = match completed.outcome {
            Ok(VoiceOutcome::Answer(answer)) => State::Answer(answer),
            Ok(VoiceOutcome::NotAvailable) => State::NotAvailable,
            Err(error) => State::Error(error),
        };
    }
}

impl<M: MicSource> Drop for PushToTalk<M> {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::SAMPLE_RATE;

    #[derive(Default)]
    struct TestMic {
        active: bool,
        start_error: bool,
        read_error: bool,
        overreports: bool,
        sample_rate: u32,
    }

    impl MicSource for TestMic {
        fn start(&mut self) -> Result<u32, MicError> {
            if self.start_error {
                return Err(MicError);
            }
            assert!(!self.active);
            self.active = true;
            Ok(self.sample_rate)
        }

        fn read_available(&mut self, output: &mut [i16]) -> Result<usize, MicError> {
            assert!(self.active);
            if self.read_error {
                return Err(MicError);
            }
            if self.overreports {
                return Ok(output.len() + 1);
            }
            output[0] = 123;
            Ok(1)
        }

        fn stop(&mut self) {
            assert!(self.active);
            self.active = false;
        }
    }

    fn recorder(mic: TestMic, flag: Option<&str>) -> PushToTalk<TestMic> {
        let client = VoiceClient::new("http://127.0.0.1:1", flag).unwrap();
        PushToTalk::new(mic, &client)
    }

    #[test]
    fn disabled_recorder_never_starts_the_microphone() {
        let mut recorder = recorder(TestMic::default(), None);
        recorder.start_recording();
        assert_eq!(recorder.state(), VoiceState::Idle);
        assert!(!recorder.mic.active);
    }

    #[test]
    fn elapsed_limit_queues_a_partial_clip_and_stops_capture() {
        let mut recorder = recorder(
            TestMic {
                sample_rate: SAMPLE_RATE,
                ..TestMic::default()
            },
            Some("1"),
        );
        recorder.start_recording();
        let State::Recording(recording) = &mut recorder.state else {
            panic!("recording did not start");
        };
        recording.started = Instant::now() - MAX_DURATION;
        let pending = recorder.poll_recording().unwrap();
        assert_eq!(pending.wav, encode_wav(&[123]).unwrap());
        assert!(!recorder.mic.active);
        assert_eq!(recorder.state(), VoiceState::Sending);
        recorder.complete(CompletedTurn {
            canceled: pending.canceled,
            outcome: Ok(VoiceOutcome::NotAvailable),
        });
        assert_eq!(recorder.state(), VoiceState::NotAvailable);
    }

    #[test]
    fn handles_start_and_read_errors_and_rejects_invalid_source_rates() {
        for (mic, expected) in [
            (
                TestMic {
                    start_error: true,
                    ..TestMic::default()
                },
                VoiceError::Microphone,
            ),
            (
                TestMic {
                    read_error: true,
                    sample_rate: SAMPLE_RATE,
                    ..TestMic::default()
                },
                VoiceError::Microphone,
            ),
            (
                TestMic {
                    overreports: true,
                    sample_rate: SAMPLE_RATE,
                    ..TestMic::default()
                },
                VoiceError::Microphone,
            ),
            (TestMic::default(), VoiceError::InvalidAudio),
        ] {
            let mut recorder = recorder(mic, Some("1"));
            recorder.start_recording();
            assert!(recorder.finish_recording().is_none());
            assert_eq!(recorder.state(), VoiceState::Error(expected));
            assert!(!recorder.mic.active);
        }
    }
}
