// Windows meeting-mode audio backend.
//
// Three jobs, mirroring CoreAudio on macOS and `pulse.rs` on Linux:
//   * `SystemAudioCapture` — record what the speakers/headset are playing, via
//     WASAPI loopback on the DEFAULT render endpoint (cpal opens a loopback
//     stream when an input stream is built on an output device). It follows
//     the default output: switching from speakers to a headset mid-call
//     reopens the stream on the new device within ~2 s.
//   * `detect_output_route` — speakers vs headphones, for the echo duck, from
//     the endpoint's form factor.
//   * `recording_apps` — which other apps are using the microphone right now,
//     for meeting auto-detection, from the privacy "consent store" Windows
//     keeps for the microphone indicator.
//
// Everything is best-effort: capture failures surface as an error the caller
// already knows how to report; detection failures mean "nothing detected".

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures_channel::mpsc;
use futures_util::{Stream, StreamExt};
use log::{debug, info, warn};

/// How often the capture thread checks whether the default output changed.
const DEFAULT_DEVICE_CHECK: Duration = Duration::from_secs(2);
/// WASAPI loopback delivers NOTHING while no app is playing audio. The capture
/// loop downstream expects a real-time stream (it mixes the system side with
/// the mic by position), so gaps longer than this are filled with silence.
const MAX_GAP: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Output route (echo mitigation)
// ---------------------------------------------------------------------------

/// Classification of the default OUTPUT device for echo mitigation. Same
/// meaning as the macOS / Linux type of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputRoute {
    /// Speakers / HDMI / DisplayPort — the mic hears the remote party.
    Speakers,
    /// Headphones / headsets — no acoustic leakage into the mic.
    Headphones,
    /// Couldn't tell; treated as speakers (mitigation on).
    Unknown,
}

/// Classify the default output as speakers vs headphones, from the endpoint
/// form factor the driver reports, falling back to its friendly name
/// ("Headphones (WH-1000XM4)", "Speakers (Realtek(R) Audio)").
pub fn detect_output_route() -> OutputRoute {
    let form_factor = default_render_form_factor();
    let name = cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok());
    let route = classify_route(form_factor, name.as_deref());
    info!(
        "meeting: output route {:?} (form factor {:?}, device {:?})",
        route, form_factor, name
    );
    route
}

/// `EndpointFormFactor` of the default render endpoint.
fn default_render_form_factor() -> Option<u32> {
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        PKEY_AudioEndpoint_FormFactor,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };

    unsafe {
        // Already initialised on this thread (by us or a library) is fine.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()?;
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(&PKEY_AudioEndpoint_FormFactor).ok()?;
        u32::try_from(&value).ok()
    }
}

fn classify_route(form_factor: Option<u32>, name: Option<&str>) -> OutputRoute {
    // EndpointFormFactor: 1 Speakers, 2 LineLevel, 3 Headphones, 4 Handset,
    // 5 Headset, 8 SPDIF, 9 DigitalAudioDisplayDevice (HDMI/DP).
    match form_factor {
        Some(3..=5) => return OutputRoute::Headphones,
        Some(1 | 8 | 9) => return OutputRoute::Speakers,
        _ => {}
    }
    if let Some(name) = name.map(str::to_lowercase) {
        if [
            "headphone",
            "headset",
            "earphone",
            "airpods",
            "buds",
            "hands-free",
        ]
        .iter()
        .any(|k| name.contains(k))
        {
            return OutputRoute::Headphones;
        }
        if ["speaker", "hdmi", "display", "monitor", "tv"]
            .iter()
            .any(|k| name.contains(k))
        {
            return OutputRoute::Speakers;
        }
    }
    OutputRoute::Unknown
}

// ---------------------------------------------------------------------------
// Microphone users (meeting auto-detection)
// ---------------------------------------------------------------------------

/// An app Windows currently lists as using the microphone.
#[derive(Clone, Debug, Default)]
pub struct RecordingApp {
    /// Executable file stem for desktop apps, e.g. "chrome", "ms-teams", "zoom".
    pub exe: Option<String>,
    /// Package family name without the publisher hash for Store / MSIX apps,
    /// e.g. "MSTeams", "5319275A.WhatsAppDesktop".
    pub package: Option<String>,
}

const MIC_CONSENT_STORE: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// Apps using the microphone right now, from the consent store that drives
/// the taskbar microphone indicator: an entry whose `LastUsedTimeStop` is 0
/// while `LastUsedTimeStart` is set is recording at this moment. Desktop apps
/// live under `NonPackaged`, keyed by their path with `\` written as `#`.
/// This process is excluded (its own meeting capture would always match).
pub fn recording_apps() -> Result<Vec<RecordingApp>> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let store = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(MIC_CONSENT_STORE)
        .map_err(|e| anyhow!("microphone consent store unavailable: {e}"))?;
    let own_exe = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "#").to_lowercase());

    let in_use = |key: &RegKey| -> bool {
        let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
        let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(1);
        start != 0 && stop == 0
    };

    let mut apps = Vec::new();
    for name in store.enum_keys().flatten() {
        if name.eq_ignore_ascii_case("NonPackaged") {
            let Ok(non_packaged) = store.open_subkey(&name) else {
                continue;
            };
            for path in non_packaged.enum_keys().flatten() {
                if own_exe.as_deref() == Some(path.to_lowercase().as_str()) {
                    continue;
                }
                if non_packaged.open_subkey(&path).is_ok_and(|k| in_use(&k)) {
                    apps.push(RecordingApp {
                        exe: exe_stem(&path),
                        package: None,
                    });
                }
            }
        } else if store.open_subkey(&name).is_ok_and(|k| in_use(&k)) {
            apps.push(RecordingApp {
                exe: None,
                package: Some(package_name(&name).to_string()),
            });
        }
    }
    Ok(apps)
}

/// "C:#Program Files#Google#Chrome#Application#chrome.exe" → "chrome".
fn exe_stem(consent_path: &str) -> Option<String> {
    let file = consent_path.rsplit(['#', '\\', '/']).next()?;
    let stem = file
        .strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file);
    (!stem.is_empty()).then(|| stem.to_string())
}

/// "MSTeams_8wekyb3d8bbwe" → "MSTeams" (drops the publisher hash).
fn package_name(family: &str) -> &str {
    family.split('_').next().unwrap_or(family)
}

/// Whether the consent store can be read at all (one-time availability probe).
pub fn mic_usage_detection_available() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(MIC_CONSENT_STORE)
        .is_ok()
}

/// Executable file stem of the process owning a window, lowercased.
fn window_process_stem(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        exe_stem(&path).map(|s| s.to_lowercase())
    }
}

/// Titles of visible top-level windows owned by a process whose executable
/// stem (lowercased) is in `exe_stems`; the foreground window comes first.
/// Used by meeting naming to read a meeting app's window / browser tab title.
pub fn window_titles_for(exe_stems: &[&str]) -> Vec<String> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    struct Ctx<'a> {
        stems: &'a [&'a str],
        foreground: HWND,
        titles: Vec<String>,
    }

    fn title_of(hwnd: HWND) -> Option<String> {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
        }
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let Some(stem) = window_process_stem(hwnd) else {
            return true.into();
        };
        if ctx.stems.contains(&stem.as_str()) {
            if let Some(title) = title_of(hwnd) {
                if hwnd == ctx.foreground {
                    ctx.titles.insert(0, title);
                } else if ctx.titles.len() < 20 {
                    ctx.titles.push(title);
                }
            }
        }
        true.into()
    }

    let mut ctx = Ctx {
        stems: exe_stems,
        foreground: unsafe { GetForegroundWindow() },
        titles: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.titles
}

// ---------------------------------------------------------------------------
// System-audio capture (WASAPI loopback)
// ---------------------------------------------------------------------------

/// System-audio capture entry point (default render endpoint loopback).
pub struct SystemAudioCapture;

impl SystemAudioCapture {
    /// Start capturing what the default output plays, as a stream of mono f32
    /// samples at the device's mix rate. Fails fast when there is no output
    /// device or the loopback stream cannot be opened.
    pub fn start() -> Result<SystemAudioStream> {
        let (tx, rx) = mpsc::unbounded::<Vec<f32>>();
        let stop = Arc::new(AtomicBool::new(false));
        let rate = Arc::new(AtomicU32::new(0));
        let stop_thread = stop.clone();
        let rate_thread = rate.clone();
        // cpal streams are not `Send` on every backend, so the stream is
        // created and owned by the capture thread, which reports whether it
        // came up.
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        std::thread::Builder::new()
            .name("meeting-system-audio".into())
            .spawn(move || match Loopback::open() {
                Ok(loopback) => {
                    rate_thread.store(loopback.rate, Ordering::Relaxed);
                    let _ = init_tx.send(Ok(()));
                    capture_thread(loopback, tx, stop_thread, rate_thread);
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                }
            })
            .map_err(|e| anyhow!("failed to spawn system capture thread: {e}"))?;
        init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow!("system audio capture did not start in time"))??;

        Ok(SystemAudioStream {
            stop,
            sample_rate: rate.load(Ordering::Relaxed),
            sample_rate_handle: rate,
            receiver: Box::pin(rx.map(futures_util::stream::iter).flatten()),
        })
    }
}

/// One open loopback stream on a specific output device.
struct Loopback {
    _stream: cpal::Stream,
    device_name: String,
    rate: u32,
    samples: std::sync::mpsc::Receiver<Vec<f32>>,
    failed: Arc<AtomicBool>,
}

impl Loopback {
    fn open() -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| anyhow!("no default audio output"))?;
        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
        let config = device
            .default_output_config()
            .map_err(|e| anyhow!("could not read output format of '{device_name}': {e}"))?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();

        let (tx, samples) = std::sync::mpsc::channel::<Vec<f32>>();
        let failed = Arc::new(AtomicBool::new(false));
        let failed_cb = failed.clone();
        let on_error = move |e: cpal::StreamError| {
            warn!("meeting: system audio stream error: {e}");
            failed_cb.store(true, Ordering::Relaxed);
        };

        macro_rules! build {
            ($t:ty) => {
                device.build_input_stream(
                    &stream_config,
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        let _ = tx.send(downmix(data, channels));
                    },
                    on_error,
                    None,
                )
            };
        }
        let stream = match format {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::I32 => build!(i32),
            cpal::SampleFormat::U16 => build!(u16),
            other => return Err(anyhow!("unsupported output sample format {other:?}")),
        }
        .map_err(|e| anyhow!("could not open loopback on '{device_name}': {e}"))?;
        stream
            .play()
            .map_err(|e| anyhow!("could not start loopback on '{device_name}': {e}"))?;
        info!("meeting: capturing system audio from '{device_name}' ({rate} Hz, {channels} ch)");

        Ok(Self {
            _stream: stream,
            device_name,
            rate,
            samples,
            failed,
        })
    }
}

/// Interleaved frames of any sample type → mono f32.
fn downmix<T>(data: &[T], channels: usize) -> Vec<f32>
where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    let channels = channels.max(1);
    data.chunks(channels)
        .map(|frame| {
            frame
                .iter()
                .map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s))
                .sum::<f32>()
                / frame.len() as f32
        })
        .collect()
}

fn capture_thread(
    mut loopback: Loopback,
    tx: mpsc::UnboundedSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    rate: Arc<AtomicU32>,
) {
    let mut clock = SilenceClock::new(loopback.rate);
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match loopback.samples.recv_timeout(Duration::from_millis(20)) {
            Ok(chunk) => {
                clock.produced(chunk.len());
                if tx.unbounded_send(chunk).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                loopback.failed.store(true, Ordering::Relaxed);
            }
        }
        if let Some(gap) = clock.silence_due() {
            if tx.unbounded_send(vec![0.0; gap]).is_err() {
                break;
            }
        }

        // Follow the default output (speakers → headset mid-call), and reopen
        // after a device error (unplugged, driver reset). Both are checked on
        // the same cadence so a device that keeps failing is not hammered.
        if last_check.elapsed() >= DEFAULT_DEVICE_CHECK {
            let failed = loopback.failed.load(Ordering::Relaxed);
            last_check = Instant::now();
            let current = cpal::default_host()
                .default_output_device()
                .and_then(|d| d.name().ok());
            if failed
                || current
                    .as_deref()
                    .is_some_and(|c| c != loopback.device_name)
            {
                match Loopback::open() {
                    Ok(next) => {
                        info!(
                            "meeting: system audio now from '{}' (was '{}')",
                            next.device_name, loopback.device_name
                        );
                        if next.rate != loopback.rate {
                            rate.store(next.rate, Ordering::Relaxed);
                        }
                        clock = SilenceClock::new(next.rate);
                        loopback = next;
                    }
                    Err(e) => debug!("meeting: could not reopen system audio: {e}"),
                }
            }
        }
    }
    info!("meeting: system audio capture stopped");
}

/// Keeps the loopback stream real-time by reporting how much silence to
/// insert when no audio has been delivered for longer than [`MAX_GAP`].
struct SilenceClock {
    start: Instant,
    rate: f64,
    produced: u64,
}

impl SilenceClock {
    fn new(rate: u32) -> Self {
        Self {
            start: Instant::now(),
            rate: rate as f64,
            produced: 0,
        }
    }

    fn produced(&mut self, samples: usize) {
        self.produced += samples as u64;
    }

    /// Samples of silence to emit now, if the stream has fallen behind the
    /// wall clock by more than [`MAX_GAP`]. Catches up to within 20 ms, so a
    /// late real buffer arriving right after does not run ahead for long.
    fn silence_due(&mut self) -> Option<usize> {
        self.silence_due_at(self.start.elapsed())
    }

    fn silence_due_at(&mut self, elapsed: Duration) -> Option<usize> {
        let expected = (elapsed.as_secs_f64() * self.rate) as u64;
        let lag = expected.saturating_sub(self.produced);
        if lag as f64 <= MAX_GAP.as_secs_f64() * self.rate {
            return None;
        }
        let pad = lag - (0.02 * self.rate) as u64;
        self.produced += pad;
        Some(pad as usize)
    }
}

/// A stream of mono f32 system-audio samples; dropping it stops the capture.
pub struct SystemAudioStream {
    stop: Arc<AtomicBool>,
    sample_rate: u32,
    sample_rate_handle: Arc<AtomicU32>,
    receiver: Pin<Box<dyn Stream<Item = f32> + Send + Sync>>,
}

impl Drop for SystemAudioStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Stream for SystemAudioStream {
    type Item = f32;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.as_mut().poll_next_unpin(cx)
    }
}

impl SystemAudioStream {
    /// Mix rate of the output device at start.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Live sample rate; changes when the default output switches to a device
    /// with a different mix rate (the capture loop rebuilds its resampler).
    pub fn sample_rate_handle(&self) -> Arc<AtomicU32> {
        self.sample_rate_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_prefers_form_factor_then_name() {
        assert_eq!(
            classify_route(Some(3), Some("Speakers")),
            OutputRoute::Headphones
        );
        assert_eq!(classify_route(Some(9), None), OutputRoute::Speakers);
        assert_eq!(
            classify_route(None, Some("Headphones (WH-1000XM4 Stereo)")),
            OutputRoute::Headphones
        );
        assert_eq!(
            classify_route(Some(0), Some("Speakers (Realtek(R) Audio)")),
            OutputRoute::Speakers
        );
        assert_eq!(classify_route(None, None), OutputRoute::Unknown);
    }

    #[test]
    fn consent_store_names() {
        assert_eq!(
            exe_stem("C:#Program Files#Google#Chrome#Application#chrome.exe").as_deref(),
            Some("chrome")
        );
        assert_eq!(
            exe_stem(r"C:\Users\a\AppData\Local\Discord\app-1.0\Discord.exe").as_deref(),
            Some("Discord")
        );
        assert_eq!(package_name("MSTeams_8wekyb3d8bbwe"), "MSTeams");
    }

    #[test]
    fn silence_fills_only_long_gaps() {
        let mut clock = SilenceClock::new(48_000);
        clock.produced(4_800); // 100 ms delivered
        assert_eq!(clock.silence_due_at(Duration::from_millis(250)), None);
        let pad = clock.silence_due_at(Duration::from_millis(400)).unwrap();
        // Caught up to 20 ms behind the wall clock.
        assert_eq!(4_800 + pad, 19_200 - 960);
        assert_eq!(clock.silence_due_at(Duration::from_millis(400)), None);
    }
}
