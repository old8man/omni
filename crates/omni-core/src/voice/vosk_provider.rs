//! Vosk offline speech-to-text provider.
//!
//! Requires the `voice-vosk` feature and a Vosk model directory at runtime.
//! Download models from: https://alphacephei.com/vosk/models
//! Recommended for Russian: vosk-model-ru or vosk-model-small-ru

#[cfg(feature = "voice-vosk")]
mod inner {
    use std::sync::mpsc as std_mpsc;

    use anyhow::{bail, Result};
    use async_trait::async_trait;

    use crate::voice::stt::{SttConnection, SttProvider, VoiceStreamCallbacks};
    use crate::voice::types::{FinalizeSource, TranscriptionResult, VoiceStreamResult};

    enum AudioMsg {
        Samples(Vec<i16>),
        Finalize,
    }

    /// STT provider backed by Vosk (offline, local, no internet required).
    pub struct VoskSttProvider {
        model_path: String,
    }

    impl VoskSttProvider {
        pub fn new(model_path: impl Into<String>) -> Self {
            Self {
                model_path: model_path.into(),
            }
        }
    }

    #[async_trait]
    impl SttProvider for VoskSttProvider {
        fn is_available(&self) -> bool {
            std::path::Path::new(&self.model_path).exists()
        }

        async fn connect(&self, callbacks: VoiceStreamCallbacks) -> Result<Box<dyn SttConnection>> {
            if !self.is_available() {
                bail!("Vosk model not found at {:?}", self.model_path);
            }

            let model_path = self.model_path.clone();
            let transcript_tx = callbacks.transcript_tx;

            let (audio_tx, audio_rx) = std_mpsc::sync_channel::<AudioMsg>(256);
            let (result_tx, result_rx) = std_mpsc::sync_channel::<String>(1);

            std::thread::spawn(move || {
                let model = match vosk::Model::new(&model_path) {
                    Some(m) => m,
                    None => {
                        let _ = result_tx.send(String::new());
                        return;
                    }
                };
                let mut rec = match vosk::Recognizer::new(&model, 16000.0) {
                    Some(r) => r,
                    None => {
                        let _ = result_tx.send(String::new());
                        return;
                    }
                };
                rec.set_words(true);

                let mut sentences: Vec<String> = Vec::new();

                loop {
                    match audio_rx.recv() {
                        Ok(AudioMsg::Samples(samples)) => {
                            match rec.accept_waveform(&samples) {
                                Ok(vosk::DecodingState::Finalized) => {
                                    // Sentence boundary reached — grab intermediate result
                                    if let Some(r) = rec.result().single() {
                                        let text = r.text.to_string();
                                        if !text.is_empty() {
                                            sentences.push(text.clone());
                                            let _ = transcript_tx.blocking_send(VoiceStreamResult {
                                                text,
                                                is_final: false,
                                            });
                                        }
                                    }
                                }
                                Ok(_) => {
                                    // Still running — send partial for real-time display
                                    let partial = rec.partial_result().partial.to_string();
                                    if !partial.is_empty() {
                                        let _ = transcript_tx.blocking_send(VoiceStreamResult {
                                            text: partial,
                                            is_final: false,
                                        });
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Vosk accept_waveform error: {e:?}");
                                }
                            }
                        }
                        Ok(AudioMsg::Finalize) => {
                            if let Some(r) = rec.final_result().single() {
                                let text = r.text.to_string();
                                if !text.is_empty() {
                                    sentences.push(text);
                                }
                            }
                            let _ = result_tx.send(sentences.join(" "));
                            break;
                        }
                        Err(_) => {
                            let _ = result_tx.send(sentences.join(" "));
                            break;
                        }
                    }
                }
            });

            Ok(Box::new(VoskSttConnection {
                audio_tx,
                result_rx: Some(result_rx),
                connected: true,
            }))
        }
    }

    pub struct VoskSttConnection {
        audio_tx: std_mpsc::SyncSender<AudioMsg>,
        // Wrapped in Option so we can move it into spawn_blocking during finalize.
        result_rx: Option<std_mpsc::Receiver<String>>,
        connected: bool,
    }

    #[async_trait]
    impl SttConnection for VoskSttConnection {
        async fn send_audio(&mut self, audio_data: &[u8]) -> Result<()> {
            if !self.connected {
                bail!("Vosk connection closed");
            }
            // Convert little-endian i16 bytes → i16 samples
            let samples: Vec<i16> = audio_data
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect();
            if !samples.is_empty() {
                self.audio_tx
                    .send(AudioMsg::Samples(samples))
                    .map_err(|_| anyhow::anyhow!("Vosk recognizer thread died"))?;
            }
            Ok(())
        }

        async fn finalize(&mut self) -> Result<TranscriptionResult> {
            self.connected = false;
            let _ = self.audio_tx.send(AudioMsg::Finalize);
            let rx = self
                .result_rx
                .take()
                .ok_or_else(|| anyhow::anyhow!("already finalized"))?;
            // Receiving from the recognizer thread is blocking — run it off the async executor.
            let text = tokio::task::spawn_blocking(move || rx.recv().unwrap_or_default()).await?;
            Ok(TranscriptionResult {
                text,
                duration_secs: 0.0,
                finalize_source: FinalizeSource::WsClose,
            })
        }

        async fn close(&mut self) {
            self.connected = false;
            // dropping audio_tx causes the recognizer thread to exit on next recv
        }

        fn is_connected(&self) -> bool {
            self.connected
        }
    }
}

#[cfg(feature = "voice-vosk")]
pub use inner::VoskSttProvider;

/// Stub when the feature is not compiled.
#[cfg(not(feature = "voice-vosk"))]
#[derive(Debug)]
pub struct VoskSttProvider;

#[cfg(not(feature = "voice-vosk"))]
impl VoskSttProvider {
    pub fn new(_model_path: impl Into<String>) -> Self {
        Self
    }
}

#[cfg(not(feature = "voice-vosk"))]
#[async_trait::async_trait]
impl crate::voice::stt::SttProvider for VoskSttProvider {
    fn is_available(&self) -> bool {
        false
    }

    async fn connect(
        &self,
        _callbacks: crate::voice::stt::VoiceStreamCallbacks,
    ) -> anyhow::Result<Box<dyn crate::voice::stt::SttConnection>> {
        anyhow::bail!("Vosk support not compiled (enable feature `voice-vosk`)")
    }
}
