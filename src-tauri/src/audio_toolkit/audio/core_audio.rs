// CoreAudio system-audio capture for macOS (meeting mode, Step 1).
//
// Ported from meetily's `audio/capture/core_audio.rs`. Uses the `cidre` crate to
// create a CoreAudio Aggregate Device + global mono Process Tap, pushes f32
// samples from an `extern "C"` audio_proc callback into a `ringbuf::HeapRb`, and
// exposes them as a `futures_util::Stream<Item = f32>` via a Waker.
//
// Output is mono f32 at the device's native rate (~48kHz).
//
// This module is macOS-only and is intentionally isolated from the existing
// dictation/recording flow. It is only compiled on macOS (see the gated `mod`
// declaration in `audio/mod.rs`).

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use anyhow::Result;
use cidre::{arc, av, cat, cf, core_audio as ca, os};
use futures_util::Stream;
use log::{error, info, warn};
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};

/// Classification of the default OUTPUT device for echo-mitigation purposes
/// (meeting mode). On SPEAKERS the mic re-captures the remote party that the
/// system tap already captured cleanly (echo / duplicated transcript); on
/// HEADPHONES there's no acoustic leakage so no mitigation is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputRoute {
    /// Built-in / external speakers, HDMI, DisplayPort, AirPlay — echo-prone.
    Speakers,
    /// Headphones / earbuds (USB / Bluetooth / BluetoothLE headset) — no
    /// acoustic leakage into the mic.
    Headphones,
    /// Couldn't determine the transport type; treat conservatively as speakers
    /// (apply mitigation) since false-ducking is safer than duplicated text.
    Unknown,
}

/// Detect the current default OUTPUT device's transport type and classify it as
/// speakers vs headphones for echo mitigation. macOS-only (CoreAudio).
///
/// NOTE: Transport type is a coarse signal. BUILT_IN is the MacBook's internal
/// speakers (echo-prone). BLUETOOTH / BLUETOOTH_LE / USB are most often headsets
/// or earbuds (no leakage), so we treat them as headphones. This can be wrong
/// for a USB/Bluetooth *speaker*, but the only downside there is occasional mic
/// ducking while remote audio plays — acceptable vs. duplicated transcript.
pub fn detect_output_route() -> OutputRoute {
    use cidre::core_audio::DeviceTransportType as T;

    let device = match ca::System::default_output_device() {
        Ok(d) => d,
        Err(e) => {
            warn!("CoreAudio: failed to get default output device for route detection: {e:?}");
            return OutputRoute::Unknown;
        }
    };
    let transport = match device.transport_type() {
        Ok(t) => t,
        Err(e) => {
            warn!("CoreAudio: failed to read output transport type: {e:?}");
            return OutputRoute::Unknown;
        }
    };

    match transport {
        // Internal + wired desktop/TV outputs: acoustic leakage into the mic.
        T::BUILT_IN | T::HDMI | T::DISPLAY_PORT | T::AIR_PLAY => OutputRoute::Speakers,
        // Personal listening devices: no leakage.
        T::USB | T::BLUETOOTH | T::BLUETOOTH_LE => OutputRoute::Headphones,
        // Anything else (PCI, FireWire, Thunderbolt, Virtual, Aggregate, …) is
        // ambiguous; be conservative and mitigate.
        _ => OutputRoute::Unknown,
    }
}

/// Waker state for async polling.
struct WakerState {
    waker: Option<Waker>,
    has_data: bool,
}

/// CoreAudio system-audio capture using an aggregate device + process tap.
pub struct CoreAudioCapture {
    tap: ca::TapGuard,
    agg_desc: arc::Retained<cf::DictionaryOf<cf::String, cf::Type>>,
}

/// CoreAudio stream that produces audio samples.
pub struct CoreAudioStream {
    consumer: HeapCons<f32>,
    _device: ca::hardware::StartedDevice<ca::AggregateDevice>,
    _ctx: Box<AudioContext>,
    _tap: ca::TapGuard,
    waker_state: Arc<Mutex<WakerState>>,
    current_sample_rate: Arc<AtomicU32>,
}

/// Audio processing context shared with the `extern "C"` callback.
struct AudioContext {
    format: arc::R<av::AudioFormat>,
    producer: HeapProd<f32>,
    waker_state: Arc<Mutex<WakerState>>,
    current_sample_rate: Arc<AtomicU32>,
    consecutive_drops: Arc<AtomicU32>,
    should_terminate: Arc<AtomicBool>,
}

impl CoreAudioCapture {
    /// Create a new CoreAudio capture for system audio.
    pub fn new() -> Result<Self> {
        info!("CoreAudio: starting capture initialization");

        // Note: Audio Capture permission (NSAudioCaptureUsageDescription) is required
        // for macOS 14.4+. The permission dialog is automatically triggered when
        // creating the CoreAudio tap. If permission is denied, the tap returns
        // silence (all zeros).

        // Get default output device.
        let output_device = ca::System::default_output_device().map_err(|e| {
            error!("CoreAudio: failed to get default output device: {:?}", e);
            anyhow::anyhow!("Failed to get default output device: {:?}", e)
        })?;

        let output_uid = output_device.uid().map_err(|e| {
            error!("CoreAudio: failed to get device UID: {:?}", e);
            anyhow::anyhow!("Failed to get device UID: {:?}", e)
        })?;

        let device_name = output_device
            .name()
            .unwrap_or_else(|_| cf::String::from_str("Unknown"));
        info!(
            "CoreAudio: default output device: '{}' (UID: {:?})",
            device_name, output_uid
        );

        // IMPORTANT: We do NOT create a sub_device dictionary here. When using a
        // tap, the tap provides all the audio we need. Including both the tap AND
        // the device creates duplicate audio (echo issue).

        // Create process tap with mono global tap, excluding no processes. A mono
        // tap is more reliable for system audio capture on macOS.
        info!("CoreAudio: creating process tap (global mono tap)");
        let tap_desc =
            ca::TapDesc::with_mono_global_tap_excluding_processes(&cidre::ns::Array::new());
        let tap = tap_desc.create_process_tap().map_err(|e| {
            error!("CoreAudio: failed to create process tap: {:?}", e);
            anyhow::anyhow!("Failed to create process tap: {:?}", e)
        })?;

        let tap_uid = tap.uid().unwrap_or_else(|_| cf::Uuid::new().to_cf_string());
        match tap.asbd() {
            Ok(asbd) => {
                info!("CoreAudio: process tap created - UID: {:?}", tap_uid);
                info!(
                    "CoreAudio: tap format - sample_rate: {} Hz, channels: {}",
                    asbd.sample_rate, asbd.channels_per_frame
                );
            }
            Err(e) => {
                warn!(
                    "CoreAudio: tap created but couldn't get format info: {:?}",
                    e
                );
            }
        }

        // Create sub-tap dictionary.
        let sub_tap = cf::DictionaryOf::with_keys_values(
            &[ca::sub_device_keys::uid()],
            &[tap.uid().unwrap().as_type_ref()],
        );

        // Create aggregate device descriptor.
        //
        // CRITICAL: Use ONLY the tap (`tap_list`), NOT the output device as a
        // `sub_device_list`. Including both causes duplicate audio capture (echo).
        // The tap alone provides all the system audio we need.
        let agg_desc = cf::DictionaryOf::with_keys_values(
            &[
                ca::aggregate_device_keys::is_private(),
                ca::aggregate_device_keys::is_stacked(),
                ca::aggregate_device_keys::tap_auto_start(),
                ca::aggregate_device_keys::name(),
                ca::aggregate_device_keys::main_sub_device(),
                ca::aggregate_device_keys::uid(),
                // REMOVED: sub_device_list (was causing duplicate audio).
                ca::aggregate_device_keys::tap_list(),
            ],
            &[
                cf::Boolean::value_true().as_type_ref(),
                cf::Boolean::value_false(),
                cf::Boolean::value_true(),
                cf::str!(c"fisilti-audio-tap").as_type_ref(),
                &output_uid,
                &cf::Uuid::new().to_cf_string(),
                // REMOVED: sub_device array (was causing echo).
                &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
            ],
        );

        info!("CoreAudio: capture initialized successfully");

        Ok(Self { tap, agg_desc })
    }

    /// Start the audio device and create the IO proc.
    fn start_device(
        &self,
        ctx: &mut Box<AudioContext>,
    ) -> Result<ca::hardware::StartedDevice<ca::AggregateDevice>> {
        extern "C" fn audio_proc(
            device: ca::Device,
            _now: &cat::AudioTimeStamp,
            input_data: &cat::AudioBufList<1>,
            _input_time: &cat::AudioTimeStamp,
            _output_data: &mut cat::AudioBufList<1>,
            _output_time: &cat::AudioTimeStamp,
            ctx: Option<&mut AudioContext>,
        ) -> os::Status {
            let ctx = ctx.unwrap();

            // Detect sample rate changes.
            let after = device
                .nominal_sample_rate()
                .unwrap_or(ctx.format.absd().sample_rate) as u32;
            let before = ctx.current_sample_rate.load(Ordering::Acquire);
            if before != after {
                ctx.current_sample_rate.store(after, Ordering::Release);
            }

            // Try to get audio data from the buffer list.
            if let Some(view) =
                av::AudioPcmBuf::with_buf_list_no_copy(&ctx.format, input_data, None)
            {
                if let Some(data) = view.data_f32_at(0) {
                    process_audio_data(ctx, data);
                }
            } else if ctx.format.common_format() == av::audio::CommonFormat::PcmF32 {
                // Fallback: manual extraction if AudioPcmBuf fails.
                let first_buffer = &input_data.buffers[0];
                let byte_count = first_buffer.data_bytes_size as usize;
                let float_count = byte_count / std::mem::size_of::<f32>();

                if float_count > 0 && first_buffer.data != std::ptr::null_mut() {
                    let data = unsafe {
                        std::slice::from_raw_parts(first_buffer.data as *const f32, float_count)
                    };
                    process_audio_data(ctx, data);
                }
            }

            os::Status::NO_ERR
        }

        info!("CoreAudio: creating aggregate device");
        let agg_device = ca::AggregateDevice::with_desc(&self.agg_desc).map_err(|e| {
            error!("CoreAudio: failed to create aggregate device: {:?}", e);
            anyhow::anyhow!("Failed to create aggregate device: {:?}", e)
        })?;

        info!("CoreAudio: creating IO proc");
        let proc_id = agg_device
            .create_io_proc_id(audio_proc, Some(ctx))
            .map_err(|e| {
                error!("CoreAudio: failed to create IO proc: {:?}", e);
                anyhow::anyhow!("Failed to create IO proc: {:?}", e)
            })?;

        info!("CoreAudio: starting audio device");
        let started_device = ca::device_start(agg_device, Some(proc_id)).map_err(|e| {
            error!("CoreAudio: failed to start device: {:?}", e);
            anyhow::anyhow!("Failed to start device: {:?}", e)
        })?;

        let device_ref = started_device.as_ref();
        let sample_rate = device_ref.nominal_sample_rate().unwrap_or(0.0);
        info!(
            "CoreAudio: aggregate device started, sample_rate: {} Hz",
            sample_rate
        );

        Ok(started_device)
    }

    /// Consume this capture and produce a stream of f32 samples.
    pub fn stream(self) -> Result<CoreAudioStream> {
        info!("CoreAudio: creating CoreAudioStream");

        let asbd = self.tap.asbd().map_err(|e| {
            error!("CoreAudio: failed to get tap ASBD: {:?}", e);
            anyhow::anyhow!("Failed to get tap ASBD: {:?}", e)
        })?;

        let format = av::AudioFormat::with_asbd(&asbd).ok_or_else(|| {
            error!("CoreAudio: failed to create audio format");
            anyhow::anyhow!("Failed to create audio format")
        })?;

        info!(
            "CoreAudio: tap audio format: {} Hz, {} channels",
            asbd.sample_rate, asbd.channels_per_frame
        );

        // Lock-free ring buffer for audio transfer (128K f32 samples).
        let buffer_size = 1024 * 128;
        let rb = HeapRb::<f32>::new(buffer_size);
        let (producer, consumer) = rb.split();

        let waker_state = Arc::new(Mutex::new(WakerState {
            waker: None,
            has_data: false,
        }));

        let current_sample_rate = Arc::new(AtomicU32::new(asbd.sample_rate as u32));

        let mut ctx = Box::new(AudioContext {
            format,
            producer,
            waker_state: waker_state.clone(),
            current_sample_rate: current_sample_rate.clone(),
            consecutive_drops: Arc::new(AtomicU32::new(0)),
            should_terminate: Arc::new(AtomicBool::new(false)),
        });

        let device = self.start_device(&mut ctx)?;

        info!("CoreAudio: CoreAudioStream created successfully");

        Ok(CoreAudioStream {
            consumer,
            _device: device,
            _ctx: ctx,
            _tap: self.tap,
            waker_state,
            current_sample_rate,
        })
    }
}

/// Push audio data from the IO proc callback into the ring buffer.
fn process_audio_data(ctx: &mut AudioContext, data: &[f32]) {
    // Push raw samples directly. Any gain/mixing is handled downstream (Step 2).
    let buffer_size = data.len();
    let pushed = ctx.producer.push_slice(data);

    if pushed < buffer_size {
        // Ring-buffer back-pressure: the consumer fell behind and we couldn't
        // push every sample this callback, so the unpushed (newest) samples are
        // dropped. Previously we hard-terminated system capture after 10
        // consecutive drops, which killed the tap for the rest of a
        // (potentially hours-long) meeting on a transient stall. Instead we
        // log-and-continue, only giving up on a truly persistent failure.
        // Losing a few ms of audio on a momentary stall is far better than
        // losing the remainder of the meeting.
        let consecutive = ctx.consecutive_drops.fetch_add(1, Ordering::AcqRel) + 1;
        let overflow = buffer_size - pushed;
        // Log sparingly so a steady-but-tolerable overrun doesn't spam the log.
        if consecutive == 1 || consecutive % 100 == 0 {
            warn!(
                "CoreAudio: ring buffer overflow (dropped {} samples, {} consecutive)",
                overflow, consecutive
            );
        }
        // Only terminate on a truly persistent, unrecoverable backlog (the
        // consumer has been stalled for a very long time). 10_000 consecutive
        // overflowing callbacks is on the order of minutes of continuous
        // failure, not a transient hiccup.
        if consecutive > 10_000 {
            error!("CoreAudio: persistent ring buffer overflow; terminating system capture");
            ctx.should_terminate.store(true, Ordering::Release);
            return;
        }
    } else {
        ctx.consecutive_drops.store(0, Ordering::Release);
    }

    if pushed > 0 {
        let should_wake = {
            let mut waker_state = ctx.waker_state.lock().unwrap();
            if !waker_state.has_data {
                waker_state.has_data = true;
                waker_state.waker.take()
            } else {
                None
            }
        };

        if let Some(waker) = should_wake {
            waker.wake();
        }
    }
}

impl CoreAudioStream {
    /// Current device sample rate (Hz).
    pub fn sample_rate(&self) -> u32 {
        self.current_sample_rate.load(Ordering::Acquire)
    }

    /// Shared handle to the live device sample rate. The IO-proc callback
    /// updates this atomically when the device rate changes (e.g. Bluetooth
    /// profile switches), so a consumer that has moved the stream into a task
    /// can still observe the current rate and rebuild its resampler (Item 4).
    pub fn sample_rate_handle(&self) -> Arc<AtomicU32> {
        self.current_sample_rate.clone()
    }
}

impl Stream for CoreAudioStream {
    type Item = f32;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(sample) = self.consumer.try_pop() {
            return Poll::Ready(Some(sample));
        }

        if self._ctx.should_terminate.load(Ordering::Acquire) {
            warn!("CoreAudio: stream terminating due to buffer pressure");
            return match self.consumer.try_pop() {
                Some(sample) => Poll::Ready(Some(sample)),
                None => Poll::Ready(None),
            };
        }

        // No data available: register waker and return pending.
        {
            let mut state = self.waker_state.lock().unwrap();
            state.has_data = false;
            state.waker = Some(cx.waker().clone());
        }

        Poll::Pending
    }
}

impl Drop for CoreAudioStream {
    fn drop(&mut self) {
        info!("CoreAudio: stream dropped, signaling termination");
        self._ctx.should_terminate.store(true, Ordering::Release);
    }
}
