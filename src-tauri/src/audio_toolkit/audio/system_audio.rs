// System-audio capture wrapper for macOS (meeting mode, Step 1).
//
// Trimmed, macOS-only port of meetily's `audio/capture/system.rs`. Wraps the
// CoreAudio tap stream behind a `SystemAudioStream` that owns a forwarding task
// and tears it down cleanly on Drop.
//
// This is isolated from handy's existing dictation/recording flow and only
// compiled on macOS (see the gated `mod` declaration in `audio/mod.rs`).

use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::Result;
use futures_channel::mpsc;
use futures_util::{Stream, StreamExt};
use log::info;

use super::core_audio::CoreAudioCapture;

/// System-audio capture entry point (CoreAudio tap on macOS).
pub struct SystemAudioCapture;

impl SystemAudioCapture {
    /// Start capturing system audio and return a stream of mono f32 samples at
    /// the device's native sample rate.
    pub fn start() -> Result<SystemAudioStream> {
        info!("Starting CoreAudio system capture (macOS)");

        let core_audio = CoreAudioCapture::new()?;
        let core_audio_stream = core_audio.stream()?;
        let sample_rate = core_audio_stream.sample_rate();

        // Forward CoreAudio samples through an unbounded channel so the consumer
        // can poll a plain `Stream<Item = f32>` without holding CoreAudio types.
        let (tx, rx) = mpsc::unbounded::<Vec<f32>>();
        let (drop_tx, drop_rx) = std::sync::mpsc::channel::<()>();

        // Use Tauri's async runtime so this works whether or not the caller is on
        // a tokio runtime.
        tauri::async_runtime::spawn(async move {
            let mut stream = core_audio_stream;
            let mut buffer = Vec::new();
            let chunk_size = 1024;

            loop {
                if drop_rx.try_recv().is_ok() {
                    break;
                }

                match stream.next().await {
                    Some(sample) => {
                        buffer.push(sample);
                        if buffer.len() >= chunk_size {
                            if tx.unbounded_send(std::mem::take(&mut buffer)).is_err() {
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }

            if !buffer.is_empty() {
                let _ = tx.unbounded_send(buffer);
            }
        });

        let receiver = rx.map(futures_util::stream::iter).flatten();

        info!("CoreAudio system capture started successfully");

        Ok(SystemAudioStream {
            drop_tx,
            sample_rate,
            receiver: Box::pin(receiver),
        })
    }
}

/// A stream of mono f32 system-audio samples with clean teardown on Drop.
pub struct SystemAudioStream {
    drop_tx: std::sync::mpsc::Sender<()>,
    sample_rate: u32,
    receiver: Pin<Box<dyn Stream<Item = f32> + Send + Sync>>,
}

impl Drop for SystemAudioStream {
    fn drop(&mut self) {
        let _ = self.drop_tx.send(());
    }
}

impl Stream for SystemAudioStream {
    type Item = f32;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.as_mut().poll_next_unpin(cx)
    }
}

impl SystemAudioStream {
    /// Native sample rate (Hz) reported by the CoreAudio tap.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
