use anyhow::{bail, Result};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::types::{FinalizeSource, TranscriptionResult, VoiceStreamResult};

/// Callbacks for a voice stream session.
pub struct VoiceStreamCallbacks {
    pub transcript_tx: mpsc::Sender<VoiceStreamResult>,
}

/// A speech-to-text provider abstraction.
#[async_trait::async_trait]
pub trait SttProvider: Send + Sync {
    /// Check whether the STT provider is available.
    fn is_available(&self) -> bool;
    /// Open a streaming connection to the STT service.
    async fn connect(&self, callbacks: VoiceStreamCallbacks) -> Result<Box<dyn SttConnection>>;
}

/// An active STT connection for a single voice input session.
#[async_trait::async_trait]
pub trait SttConnection: Send {
    /// Send PCM audio data (16-bit signed, mono, 16kHz).
    async fn send_audio(&mut self, audio_data: &[u8]) -> Result<()>;
    /// Signal audio is complete and get the final transcript.
    async fn finalize(&mut self) -> Result<TranscriptionResult>;
    /// Close the connection.
    async fn close(&mut self);
    /// Whether the connection is still active.
    fn is_connected(&self) -> bool;
}

/// A process-based STT provider that runs a command accepting PCM on stdin.
pub struct ProcessSttProvider {
    command: String,
    args: Vec<String>,
}

impl ProcessSttProvider {
    /// Create a new process-based STT provider.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }
}

#[async_trait::async_trait]
impl SttProvider for ProcessSttProvider {
    fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }

    async fn connect(&self, callbacks: VoiceStreamCallbacks) -> Result<Box<dyn SttConnection>> {
        if !self.is_available() {
            bail!("STT command {:?} not found on PATH", self.command);
        }
        let mut child = tokio::process::Command::new(&self.command)
            .args(&self.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to capture stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to capture stdout"))?;

        let tx = callbacks.transcript_tx;
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty()
                    && tx
                        .send(VoiceStreamResult {
                            text: trimmed,
                            is_final: true,
                        })
                        .await
                        .is_err()
                {
                    break;
                }
            }
        });

        Ok(Box::new(ProcessSttConnection {
            child,
            stdin: Some(stdin),
            connected: true,
        }))
    }
}

struct ProcessSttConnection {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    connected: bool,
}

#[async_trait::async_trait]
impl SttConnection for ProcessSttConnection {
    async fn send_audio(&mut self, audio_data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        if let Some(ref mut stdin) = self.stdin {
            stdin.write_all(audio_data).await?;
            stdin.flush().await?;
            Ok(())
        } else {
            bail!("STT connection closed");
        }
    }

    async fn finalize(&mut self) -> Result<TranscriptionResult> {
        self.stdin = None;
        let status = self.child.wait().await?;
        self.connected = false;
        if !status.success() {
            warn!("STT process exited with status {}", status);
        }
        debug!("STT process completed");
        Ok(TranscriptionResult {
            text: String::new(),
            duration_secs: 0.0,
            finalize_source: FinalizeSource::WsClose,
        })
    }

    async fn close(&mut self) {
        self.stdin = None;
        self.connected = false;
        let _ = self.child.kill().await;
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
