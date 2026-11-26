//! Cloud-only voice turns for the Stardust push-to-talk panel.

mod audio;
mod client;
mod session;

pub use audio::{encode_wav, resample_to_16khz, MAX_RECORDING_SECONDS, MAX_WAV_BYTES, SAMPLE_RATE};
pub use client::{
    VoiceAnswer, VoiceCitation, VoiceClient, VoiceOutcome, MAX_RESPONSE_BYTES, PATH_VOICE_TURN,
};
pub use session::{CompletedTurn, MicError, MicSource, PendingTurn, PushToTalk, VoiceState};

pub const VOICE_FLAG: &str = "STARDUST_VOICE_V1";
pub const NOT_AVAILABLE_MESSAGE: &str = "Voice is not available yet";

/// The draft client is opt-in per process.
pub fn enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

pub fn enabled_from_env() -> bool {
    enabled(std::env::var(VOICE_FLAG).ok().as_deref())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VoiceError {
    #[error("Invalid ORBIT server origin")]
    ServerBase,
    #[error("Invalid voice audio")]
    InvalidAudio,
    #[error("Voice recording is too long")]
    RecordingTooLong,
    #[error("Microphone capture failed")]
    Microphone,
    #[error("Voice authorization failed")]
    Unauthorized,
    #[error("Invalid voice response")]
    BadResponse,
    #[error("Voice response is too large")]
    ResponseTooLarge,
}
