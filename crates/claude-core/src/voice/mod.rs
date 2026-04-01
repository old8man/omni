pub mod manager;
pub mod stt;
/// Voice input support: audio recording, speech-to-text, and transcription management.
pub mod types;

pub use manager::VoiceManager;
pub use stt::{ProcessSttProvider, SttConnection, SttProvider};
pub use types::{FinalizeSource, TranscriptionResult, VoiceConfig, VoiceState, VoiceStreamResult};
