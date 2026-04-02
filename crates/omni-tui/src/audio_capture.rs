//! Microphone audio capture via cpal.
//!
//! Captures from the default input device, converts to mono 16kHz i16 PCM,
//! and feeds chunks to the VoiceManager audio sender.

use anyhow::{bail, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

/// A running audio capture session.
/// Drop this to stop capturing.
pub struct AudioCapture {
    _stream: cpal::Stream,
}

/// Start capturing from the default input device.
///
/// Returns an `AudioCapture` guard (drop to stop) plus nothing — audio is sent
/// directly through `audio_tx` (which came from `VoiceManager::start_recording`).
pub fn start_capture(audio_tx: mpsc::Sender<Vec<u8>>) -> Result<AudioCapture> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no input audio device found"))?;

    // Prefer 16kHz mono i16; fall back to whatever is available then resample.
    let config = device.default_input_config()?;
    tracing::info!(
        "audio capture: sample_rate={} channels={} format={:?}",
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    );

    let native_sample_rate = config.sample_rate();
    let native_channels = config.channels() as usize;

    let stream = match config.sample_format() {
        cpal::SampleFormat::I16 => build_stream::<i16>(
            &device,
            &config.into(),
            audio_tx,
            native_sample_rate,
            native_channels,
        )?,
        cpal::SampleFormat::F32 => build_stream::<f32>(
            &device,
            &config.into(),
            audio_tx,
            native_sample_rate,
            native_channels,
        )?,
        cpal::SampleFormat::U8 => build_stream::<u8>(
            &device,
            &config.into(),
            audio_tx,
            native_sample_rate,
            native_channels,
        )?,
        _ => bail!("unsupported sample format {:?}", config.sample_format()),
    };

    stream.play()?;
    Ok(AudioCapture { _stream: stream })
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    audio_tx: mpsc::Sender<Vec<u8>>,
    native_rate: u32,
    channels: usize,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + ToI16,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // Mix down to mono
            let mono: Vec<i16> = data
                .chunks(channels)
                .map(|frame| {
                    let sum: i32 = frame.iter().map(|s| s.to_i16() as i32).sum();
                    (sum / channels as i32) as i16
                })
                .collect();

            // Resample to 16kHz if needed (simple linear interpolation)
            let resampled = if native_rate != 16000 {
                resample_linear(&mono, native_rate, 16000)
            } else {
                mono
            };

            // Convert i16 samples to little-endian bytes
            let bytes: Vec<u8> = resampled
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect();

            if !bytes.is_empty() {
                // Non-blocking: drop chunks if receiver is full (avoids blocking audio thread)
                let _ = audio_tx.try_send(bytes);
            }
        },
        |err| tracing::error!("audio capture error: {err}"),
        None,
    )?;
    Ok(stream)
}

/// Linear resampling from `from_rate` to `to_rate`.
fn resample_linear(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let idx = src as usize;
        let frac = src - idx as f64;
        let a = input.get(idx).copied().unwrap_or(0) as f64;
        let b = input.get(idx + 1).copied().unwrap_or(0) as f64;
        output.push((a + frac * (b - a)) as i16);
    }
    output
}

/// Trait for converting various sample formats to i16.
pub trait ToI16 {
    fn to_i16(self) -> i16;
}

impl ToI16 for i16 {
    fn to_i16(self) -> i16 { self }
}

impl ToI16 for f32 {
    fn to_i16(self) -> i16 {
        (self.clamp(-1.0, 1.0) * 32767.0) as i16
    }
}

impl ToI16 for u8 {
    fn to_i16(self) -> i16 {
        (self as i16 - 128) * 256
    }
}
