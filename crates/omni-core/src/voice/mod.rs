pub mod manager;
pub mod sherpa_provider;
pub mod stt;
/// Voice input support: audio recording, speech-to-text, and transcription management.
pub mod types;
pub mod vosk_provider;

pub use manager::VoiceManager;
#[cfg(feature = "voice-sherpa")]
pub use sherpa_provider::SherpaOnnxSttProvider;
pub use stt::{ProcessSttProvider, SttConnection, SttProvider};
pub use types::{
    FinalizeSource, SttProviderKind, TranscriptionResult, VoiceConfig, VoiceState,
    VoiceStreamResult,
};
pub use vosk_provider::VoskSttProvider;
