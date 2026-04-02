//! Sherpa-ONNX offline speech-to-text provider.
//!
//! Requires the `voice-sherpa` feature and a compatible model.
//! Supports Whisper, Paraformer, Zipformer, and other ONNX models.
//! For Russian: use whisper-medium or whisper-large-v2 ONNX models.
//!
//! Download from: https://github.com/k2-fsa/sherpa-onnx/releases

#[cfg(feature = "voice-sherpa")]
mod inner {
    use std::sync::mpsc as std_mpsc;

    use anyhow::{bail, Result};
    use async_trait::async_trait;

    use crate::voice::stt::{SttConnection, SttProvider, VoiceStreamCallbacks};
    use crate::voice::types::{FinalizeSource, TranscriptionResult, VoiceStreamResult};

    enum AudioMsg {
        Samples(Vec<f32>),
        Finalize,
    }

    /// Speech-to-text provider backed by Sherpa-ONNX (offline, ONNX models).
    pub struct SherpaOnnxSttProvider {
        /// Path to directory containing the ONNX model files.
        model_path: String,
        /// Language hint (e.g. "ru", "en"). Used for Whisper models.
        language: String,
    }

    impl SherpaOnnxSttProvider {
        pub fn new(model_path: impl Into<String>, language: impl Into<String>) -> Self {
            Self {
                model_path: model_path.into(),
                language: language.into(),
            }
        }
    }

    #[async_trait]
    impl SttProvider for SherpaOnnxSttProvider {
        fn is_available(&self) -> bool {
            std::path::Path::new(&self.model_path).exists()
        }

        async fn connect(&self, callbacks: VoiceStreamCallbacks) -> Result<Box<dyn SttConnection>> {
            if !self.is_available() {
                bail!("Sherpa-ONNX model not found at {:?}", self.model_path);
            }

            let model_path = self.model_path.clone();
            let language = self.language.clone();
            let transcript_tx = callbacks.transcript_tx;

            let (audio_tx, audio_rx) = std_mpsc::sync_channel::<AudioMsg>(256);
            let (result_tx, result_rx) = std_mpsc::sync_channel::<String>(1);

            std::thread::spawn(move || {
                // Build an online (streaming) recognizer config using Zipformer or Transducer model.
                // Adjust encoder/decoder paths based on model directory structure.
                let encoder = format!("{}/encoder.onnx", model_path);
                let decoder = format!("{}/decoder.onnx", model_path);
                let joiner  = format!("{}/joiner.onnx", model_path);
                let tokens  = format!("{}/tokens.txt",  model_path);

                let model_config = sherpa_rs::OnlineTransducerModelConfig {
                    encoder: encoder.into(),
                    decoder: decoder.into(),
                    joiner:  joiner.into(),
                };

                let feat_config = sherpa_rs::FeatureConfig {
                    sample_rate: 16000,
                    feature_dim: 80,
                };

                let config = sherpa_rs::OnlineRecognizerConfig {
                    feat_config,
                    model_config,
                    decoding_method: "greedy_search".into(),
                    max_active_paths: 4,
                    enable_endpoint: true,
                    rule1_min_trailing_silence: 2.4,
                    rule2_min_trailing_silence: 1.2,
                    rule3_min_utterance_length: 300.0,
                    tokens: tokens.into(),
                    ..Default::default()
                };

                let recognizer = match sherpa_rs::OnlineRecognizer::new(&config) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("failed to create Sherpa-ONNX recognizer: {e}");
                        let _ = result_tx.send(String::new());
                        return;
                    }
                };

                let mut stream = recognizer.create_stream();
                let mut sentences: Vec<String> = Vec::new();
                let mut last_text = String::new();

                loop {
                    match audio_rx.recv() {
                        Ok(AudioMsg::Samples(samples)) => {
                            stream.accept_waveform(16000, &samples);
                            while recognizer.is_ready(&stream) {
                                recognizer.decode(&stream);
                            }
                            let result = recognizer.get_result(&stream);
                            let text = result.text.trim().to_string();
                            if !text.is_empty() && text != last_text {
                                last_text = text.clone();
                                let _ = transcript_tx.blocking_send(VoiceStreamResult {
                                    text,
                                    is_final: false,
                                });
                            }
                            // Check for endpoint (sentence boundary)
                            if recognizer.is_endpoint(&stream) {
                                let final_text = last_text.clone();
                                if !final_text.is_empty() {
                                    sentences.push(final_text);
                                    last_text = String::new();
                                }
                                recognizer.reset(&stream);
                            }
                        }
                        Ok(AudioMsg::Finalize) => {
                            // Flush remaining audio
                            while recognizer.is_ready(&stream) {
                                recognizer.decode(&stream);
                            }
                            let result = recognizer.get_result(&stream);
                            let text = result.text.trim().to_string();
                            if !text.is_empty() {
                                sentences.push(text);
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

            Ok(Box::new(SherpaOnnxSttConnection {
                audio_tx,
                result_rx: Some(result_rx),
                connected: true,
            }))
        }
    }

    pub struct SherpaOnnxSttConnection {
        audio_tx: std_mpsc::SyncSender<AudioMsg>,
        result_rx: Option<std_mpsc::Receiver<String>>,
        connected: bool,
    }

    #[async_trait]
    impl SttConnection for SherpaOnnxSttConnection {
        async fn send_audio(&mut self, audio_data: &[u8]) -> Result<()> {
            if !self.connected {
                bail!("Sherpa-ONNX connection closed");
            }
            // Convert little-endian i16 bytes → normalized f32 samples
            let samples: Vec<f32> = audio_data
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect();
            if !samples.is_empty() {
                self.audio_tx
                    .send(AudioMsg::Samples(samples))
                    .map_err(|_| anyhow::anyhow!("Sherpa-ONNX recognizer thread died"))?;
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
            let text =
                tokio::task::spawn_blocking(move || rx.recv().unwrap_or_default()).await?;
            Ok(TranscriptionResult {
                text,
                duration_secs: 0.0,
                finalize_source: FinalizeSource::WsClose,
            })
        }

        async fn close(&mut self) {
            self.connected = false;
        }

        fn is_connected(&self) -> bool {
            self.connected
        }
    }
}

#[cfg(feature = "voice-sherpa")]
pub use inner::SherpaOnnxSttProvider;

/// Stub when the feature is not compiled.
#[cfg(not(feature = "voice-sherpa"))]
#[derive(Debug)]
pub struct SherpaOnnxSttProvider;

#[cfg(not(feature = "voice-sherpa"))]
impl SherpaOnnxSttProvider {
    pub fn new(_model_path: impl Into<String>, _language: impl Into<String>) -> Self {
        Self
    }
}

#[cfg(not(feature = "voice-sherpa"))]
#[async_trait::async_trait]
impl crate::voice::stt::SttProvider for SherpaOnnxSttProvider {
    fn is_available(&self) -> bool {
        false
    }

    async fn connect(
        &self,
        _callbacks: crate::voice::stt::VoiceStreamCallbacks,
    ) -> anyhow::Result<Box<dyn crate::voice::stt::SttConnection>> {
        anyhow::bail!("Sherpa-ONNX support not compiled (enable feature `voice-sherpa`)")
    }
}
