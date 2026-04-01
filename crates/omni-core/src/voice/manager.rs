use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Result};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info};

use super::stt::{SttConnection, SttProvider, VoiceStreamCallbacks};
use super::types::{TranscriptionResult, VoiceConfig, VoiceState, VoiceStreamResult};

/// Manages voice input: recording, STT integration, transcription delivery.
pub struct VoiceManager {
    config: VoiceConfig,
    state: Arc<RwLock<VoiceState>>,
    provider: Arc<dyn SttProvider>,
    connection: Arc<Mutex<Option<Box<dyn SttConnection>>>>,
    transcript_rx: Arc<Mutex<Option<mpsc::Receiver<VoiceStreamResult>>>>,
    recording_start: Arc<Mutex<Option<Instant>>>,
}

impl VoiceManager {
    /// Create a new voice manager with the given STT provider.
    pub fn new(config: VoiceConfig, provider: Arc<dyn SttProvider>) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(VoiceState::Idle)),
            provider,
            connection: Arc::new(Mutex::new(None)),
            transcript_rx: Arc::new(Mutex::new(None)),
            recording_start: Arc::new(Mutex::new(None)),
        }
    }

    /// Current voice input state.
    pub async fn state(&self) -> VoiceState {
        self.state.read().await.clone()
    }

    /// Whether voice input is available.
    pub fn is_available(&self) -> bool {
        self.config.enabled && self.provider.is_available()
    }

    /// Start voice input recording. Returns a sender for audio chunks.
    pub async fn start_recording(&self) -> Result<mpsc::Sender<Vec<u8>>> {
        if *self.state.read().await == VoiceState::Recording {
            bail!("already recording");
        }
        if !self.is_available() {
            bail!("voice input is not available");
        }

        let (transcript_tx, transcript_rx) = mpsc::channel(64);
        let connection = self
            .provider
            .connect(VoiceStreamCallbacks { transcript_tx })
            .await?;
        *self.connection.lock().await = Some(connection);
        *self.transcript_rx.lock().await = Some(transcript_rx);
        *self.recording_start.lock().await = Some(Instant::now());
        *self.state.write().await = VoiceState::Recording;

        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(128);
        let conn = self.connection.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            while let Some(chunk) = audio_rx.recv().await {
                let mut guard = conn.lock().await;
                if let Some(ref mut c) = *guard {
                    if let Err(e) = c.send_audio(&chunk).await {
                        *state.write().await = VoiceState::Error(format!("audio send error: {e}"));
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        info!("voice recording started");
        Ok(audio_tx)
    }

    /// Stop recording and get the final transcription.
    pub async fn stop_recording(&self) -> Result<TranscriptionResult> {
        if *self.state.read().await != VoiceState::Recording {
            bail!("not currently recording");
        }
        *self.state.write().await = VoiceState::Processing;

        let duration = self
            .recording_start
            .lock()
            .await
            .map(|s| s.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        let mut result = {
            let mut guard = self.connection.lock().await;
            if let Some(mut c) = guard.take() {
                match c.finalize().await {
                    Ok(mut r) => {
                        r.duration_secs = duration;
                        Ok(r)
                    }
                    Err(e) => {
                        *self.state.write().await = VoiceState::Error(format!("{e}"));
                        Err(e)
                    }
                }
            } else {
                bail!("no active STT connection");
            }
        };

        if let Ok(ref mut transcription) = result {
            let mut rx_guard = self.transcript_rx.lock().await;
            if let Some(ref mut rx) = *rx_guard {
                let mut collected = Vec::new();
                while let Ok(chunk) = rx.try_recv() {
                    collected.push(chunk.text);
                }
                if !collected.is_empty() && transcription.text.is_empty() {
                    transcription.text = collected.join(" ");
                }
            }
            *rx_guard = None;
        }

        *self.recording_start.lock().await = None;
        *self.state.write().await = VoiceState::Idle;
        info!(duration_secs = duration, "voice recording stopped");
        result
    }

    /// Cancel recording without transcription.
    pub async fn cancel_recording(&self) {
        if let Some(mut c) = self.connection.lock().await.take() {
            c.close().await;
        }
        *self.transcript_rx.lock().await = None;
        *self.recording_start.lock().await = None;
        *self.state.write().await = VoiceState::Idle;
        debug!("voice recording cancelled");
    }

    /// Get the next available transcript chunk without blocking.
    pub async fn try_recv_transcript(&self) -> Option<VoiceStreamResult> {
        self.transcript_rx
            .lock()
            .await
            .as_mut()
            .and_then(|rx| rx.try_recv().ok())
    }
}
