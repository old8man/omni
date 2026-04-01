use serde::{Deserialize, Serialize};

/// Configuration for voice input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    pub enabled: bool,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_channels")]
    pub channels: u16,
    #[serde(default = "default_silence_duration")]
    pub silence_duration_secs: f32,
    #[serde(default = "default_silence_threshold")]
    pub silence_threshold_pct: f32,
}

fn default_sample_rate() -> u32 {
    16000
}
fn default_channels() -> u16 {
    1
}
fn default_silence_duration() -> f32 {
    2.0
}
fn default_silence_threshold() -> f32 {
    3.0
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sample_rate: 16000,
            channels: 1,
            silence_duration_secs: 2.0,
            silence_threshold_pct: 3.0,
        }
    }
}

/// Result from a speech-to-text transcription chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStreamResult {
    pub text: String,
    pub is_final: bool,
}

/// A complete transcription result after voice input is finalized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub duration_secs: f32,
    pub finalize_source: FinalizeSource,
}

/// How a voice stream transcription was finalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinalizeSource {
    PostCloseStreamEndpoint,
    NoDataTimeout,
    SafetyTimeout,
    WsClose,
    WsAlreadyClosed,
}

/// State of the voice input system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceState {
    Idle,
    Recording,
    Processing,
    Error(String),
}
