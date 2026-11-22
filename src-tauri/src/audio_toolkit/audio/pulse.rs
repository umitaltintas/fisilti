// Linux meeting-mode audio backend, over the PulseAudio API.
//
// Modern distros run PipeWire with `pipewire-pulse`, which serves this API, so
// one implementation covers both PipeWire and classic PulseAudio desktops.
//
// Three jobs, mirroring what CoreAudio does on macOS:
//   * `SystemAudioCapture` — record what the speakers/headset are playing, from
//     the DEFAULT sink's monitor source. Unlike the macOS tap it follows the
//     default output: switching from speakers to a headset mid-call reconnects
//     to the new device's monitor within ~2 s instead of recording silence.
//   * `detect_output_route` — speakers vs headphones, for the echo duck.
//   * `recording_apps` — which other processes are pulling microphone input
//     right now, for meeting auto-detection.
//
// Everything is best-effort: no PulseAudio server means an error the caller
// already knows how to surface (capture) or "nothing detected" (polling).

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures_channel::mpsc;
use futures_util::{Stream, StreamExt};
use libpulse_binding as pulse;
use libpulse_simple_binding::Simple;
use log::{info, warn};
use pulse::callbacks::ListResult;
use pulse::context::{Context as PaContext, FlagSet, State as ContextState};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::operation::{Operation, State as OpState};
use pulse::proplist::Proplist;

const APP_NAME: &str = "Fısıltı";

/// Rate the system stream is requested at; the server resamples to it, so it
/// never changes mid-session (the rate handle exists for API parity with macOS).
const SYSTEM_RATE: u32 = 48_000;
/// Samples per read: 20 ms. Small enough that the stop flag and the
/// default-device check are noticed promptly.
const READ_SAMPLES: usize = (SYSTEM_RATE as usize) / 50;
/// How often the capture thread checks whether the default output changed.
const DEFAULT_SINK_CHECK: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Introspection (blocking, on a private standard mainloop)
// ---------------------------------------------------------------------------

/// A short-lived connection to the sound server for synchronous queries.
struct Introspect {
    // Field order matters: the context must be dropped BEFORE the mainloop it
    // was created on (libpulse aborts otherwise). See also `Drop` below.
    context: PaContext,
    mainloop: Mainloop,
}

impl Drop for Introspect {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

impl Introspect {
    fn connect() -> Result<Self> {
        let mut proplist = Proplist::new().ok_or_else(|| anyhow!("proplist alloc failed"))?;
        let _ = proplist.set_str(pulse::proplist::properties::APPLICATION_NAME, APP_NAME);
        let mut mainloop = Mainloop::new().ok_or_else(|| anyhow!("mainloop alloc failed"))?;
        let mut context = PaContext::new_with_proplist(&mainloop, APP_NAME, &proplist)
            .ok_or_else(|| anyhow!("context alloc failed"))?;
        context
            .connect(None, FlagSet::NOAUTOSPAWN, None)
            .map_err(|e| anyhow!("connect to sound server failed: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let IterateResult::Quit(_) | IterateResult::Err(_) = mainloop.iterate(false) {
                return Err(anyhow!("sound server mainloop failed"));
            }
            match context.get_state() {
                ContextState::Ready => break,
                ContextState::Failed | ContextState::Terminated => {
                    return Err(anyhow!("sound server connection failed"))
                }
                _ => {}
            }
            if Instant::now() > deadline {
                return Err(anyhow!("timed out connecting to the sound server"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Self { context, mainloop })
    }

    /// Drive the mainloop until `op` completes (bounded).
    fn wait<T: ?Sized>(&mut self, mut op: Operation<T>) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while op.get_state() == OpState::Running {
            if let IterateResult::Quit(_) | IterateResult::Err(_) = self.mainloop.iterate(false) {
                return Err(anyhow!("sound server mainloop failed"));
            }
            if Instant::now() > deadline {
                op.cancel();
                return Err(anyhow!("sound server query timed out"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn default_sink_name(&mut self) -> Result<Option<String>> {
        let out = Rc::new(RefCell::new(None));
        let sink = out.clone();
        let op = self.context.introspect().get_server_info(move |info| {
            *sink.borrow_mut() = info.default_sink_name.as_ref().map(|s| s.to_string());
        });
        self.wait(op)?;
        let name = out.borrow_mut().take();
        Ok(name)
    }

    fn sink(&mut self, name: &str) -> Result<Option<SinkSummary>> {
        let out = Rc::new(RefCell::new(None));
        let sink = out.clone();
        let op = self
            .context
            .introspect()
            .get_sink_info_by_name(name, move |result| {
                if let ListResult::Item(info) = result {
                    *sink.borrow_mut() = Some(SinkSummary {
                        monitor_source: info.monitor_source_name.as_ref().map(|s| s.to_string()),
                        active_port: info
                            .active_port
                            .as_ref()
                            .and_then(|p| p.name.as_ref().map(|s| s.to_string())),
                        form_factor: info.proplist.get_str("device.form_factor"),
                        bus: info.proplist.get_str("device.bus"),
                        icon: info.proplist.get_str("device.icon_name"),
                    });
                }
            });
        self.wait(op)?;
        let summary = out.borrow_mut().take();
        Ok(summary)
    }

    /// Indexes of monitor sources (capturing playback, not a microphone).
    fn monitor_source_indexes(&mut self) -> Result<Vec<u32>> {
        let out = Rc::new(RefCell::new(Vec::new()));
        let list = out.clone();
        let op = self
            .context
            .introspect()
            .get_source_info_list(move |result| {
                if let ListResult::Item(info) = result {
                    if info.monitor_of_sink.is_some() {
                        list.borrow_mut().push(info.index);
                    }
                }
            });
        self.wait(op)?;
        let indexes = out.borrow().clone();
        Ok(indexes)
    }

    fn source_outputs(&mut self) -> Result<Vec<(u32, RecordingApp)>> {
        let out = Rc::new(RefCell::new(Vec::new()));
        let list = out.clone();
        let op = self
            .context
            .introspect()
            .get_source_output_info_list(move |result| {
                if let ListResult::Item(info) = result {
                    if info.corked {
                        return;
                    }
                    let props = &info.proplist;
                    list.borrow_mut().push((
                        info.source,
                        RecordingApp {
                            pid: props
                                .get_str("application.process.id")
                                .and_then(|p| p.parse().ok()),
                            binary: props.get_str("application.process.binary"),
                            name: props.get_str("application.name"),
                            app_id: props
                                .get_str("pipewire.access.portal.app_id")
                                .or_else(|| props.get_str("application.id")),
                        },
                    ));
                }
            });
        self.wait(op)?;
        let outputs = out.borrow().clone();
        Ok(outputs)
    }
}

struct SinkSummary {
    monitor_source: Option<String>,
    active_port: Option<String>,
    form_factor: Option<String>,
    bus: Option<String>,
    icon: Option<String>,
}

// ---------------------------------------------------------------------------
// Output route (echo mitigation)
// ---------------------------------------------------------------------------

/// Classification of the default OUTPUT device for echo mitigation. Same
/// meaning as the macOS type of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputRoute {
    /// Speakers / HDMI / DisplayPort — the mic hears the remote party.
    Speakers,
    /// Headphones / headsets — no acoustic leakage into the mic.
    Headphones,
    /// Couldn't tell; treated as speakers (mitigation on).
    Unknown,
}

/// Classify the default output as speakers vs headphones.
///
/// Uses, in order: the device form factor (set by ALSA UCM and BlueZ), the
/// active port name ("analog-output-headphones" vs "analog-output-speaker",
/// "hdmi-output-0"), and the bus (Bluetooth outputs are nearly always
/// headsets/earbuds). This is finer-grained than the macOS transport-type
/// heuristic: a laptop's headphone jack is recognised as headphones.
pub fn detect_output_route() -> OutputRoute {
    let summary = Introspect::connect().and_then(|mut c| {
        let Some(name) = c.default_sink_name()? else {
            return Ok(None);
        };
        c.sink(&name)
    });
    match summary {
        Ok(Some(sink)) => {
            let route = classify_route(
                sink.form_factor.as_deref(),
                sink.active_port.as_deref(),
                sink.bus.as_deref(),
                sink.icon.as_deref(),
            );
            info!(
                "meeting: output route {:?} (form factor {:?}, port {:?}, bus {:?})",
                route, sink.form_factor, sink.active_port, sink.bus
            );
            route
        }
        Ok(None) => OutputRoute::Unknown,
        Err(e) => {
            warn!("meeting: could not detect output route: {e}");
            OutputRoute::Unknown
        }
    }
}

fn classify_route(
    form_factor: Option<&str>,
    port: Option<&str>,
    bus: Option<&str>,
    icon: Option<&str>,
) -> OutputRoute {
    match form_factor {
        Some("headphone" | "headset" | "hands-free" | "handset") => return OutputRoute::Headphones,
        Some("speaker" | "tv" | "hifi" | "car" | "portable") => return OutputRoute::Speakers,
        _ => {}
    }
    if let Some(port) = port.map(str::to_ascii_lowercase) {
        if port.contains("headphone") || port.contains("headset") {
            return OutputRoute::Headphones;
        }
        if port.contains("speaker") || port.contains("hdmi") || port.contains("iec958") {
            return OutputRoute::Speakers;
        }
    }
    if bus == Some("bluetooth") {
        return OutputRoute::Headphones;
    }
    if let Some(icon) = icon {
        if icon.contains("headset") || icon.contains("headphone") {
            return OutputRoute::Headphones;
        }
    }
    OutputRoute::Unknown
}

// ---------------------------------------------------------------------------
// Microphone users (meeting auto-detection)
// ---------------------------------------------------------------------------

/// A process currently recording from a real input (not a monitor source).
#[derive(Clone, Debug, Default)]
pub struct RecordingApp {
    pub pid: Option<u32>,
    /// Executable name, e.g. "firefox", "zoom", "chrome".
    pub binary: Option<String>,
    /// Client name, e.g. "Firefox", "Google Chrome", "ZOOM VoiceEngine".
    pub name: Option<String>,
    /// Flatpak / portal app id when sandboxed, e.g. "us.zoom.Zoom".
    pub app_id: Option<String>,
}

/// Other processes pulling microphone input right now. Corked (paused)
/// streams and captures of monitor sources (screen recorders, our own system
/// capture) are excluded, as is this process.
pub fn recording_apps() -> Result<Vec<RecordingApp>> {
    let mut conn = Introspect::connect()?;
    let monitors = conn.monitor_source_indexes()?;
    let own_pid = std::process::id();
    Ok(conn
        .source_outputs()?
        .into_iter()
        .filter(|(source, app)| !monitors.contains(source) && app.pid != Some(own_pid))
        .map(|(_, app)| app)
        .collect())
}

/// Whether a sound server answers at all (one-time availability probe).
pub fn sound_server_available() -> bool {
    Introspect::connect().is_ok()
}

// ---------------------------------------------------------------------------
// System-audio capture
// ---------------------------------------------------------------------------

/// System-audio capture entry point (default sink monitor on Linux).
pub struct SystemAudioCapture;

impl SystemAudioCapture {
    /// Start capturing what the default output plays, as a stream of mono f32
    /// samples at [`SYSTEM_RATE`]. Fails fast when no sound server answers or
    /// the monitor cannot be opened.
    pub fn start() -> Result<SystemAudioStream> {
        let (tx, rx) = mpsc::unbounded::<Vec<f32>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        // The sound-server handles are not `Send`, so they are created on the
        // capture thread itself; it reports whether they came up.
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        std::thread::Builder::new()
            .name("meeting-system-audio".into())
            .spawn(move || {
                let opened = Introspect::connect().and_then(|mut conn| {
                    let (sink, monitor) = default_monitor(&mut conn)?;
                    let stream = open_record(&monitor)?;
                    info!(
                        "meeting: capturing system audio from '{}' ({} Hz mono)",
                        monitor, SYSTEM_RATE
                    );
                    Ok((conn, sink, stream))
                });
                match opened {
                    Ok((conn, sink, stream)) => {
                        let _ = init_tx.send(Ok(()));
                        capture_thread(conn, sink, stream, tx, stop_thread);
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| anyhow!("failed to spawn system capture thread: {e}"))?;
        init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow!("system audio capture did not start in time"))??;

        Ok(SystemAudioStream {
            stop,
            sample_rate_handle: Arc::new(AtomicU32::new(SYSTEM_RATE)),
            receiver: Box::pin(rx.map(futures_util::stream::iter).flatten()),
        })
    }
}

/// The default sink's name and its monitor source.
fn default_monitor(conn: &mut Introspect) -> Result<(String, String)> {
    let sink = conn
        .default_sink_name()?
        .ok_or_else(|| anyhow!("no default audio output"))?;
    let monitor = conn
        .sink(&sink)?
        .and_then(|s| s.monitor_source)
        .unwrap_or_else(|| format!("{sink}.monitor"));
    Ok((sink, monitor))
}

fn open_record(source: &str) -> Result<Simple> {
    let spec = pulse::sample::Spec {
        format: pulse::sample::Format::FLOAT32NE,
        channels: 1,
        rate: SYSTEM_RATE,
    };
    // Ask for small fragments so reads return every ~20 ms; everything else
    // is left to the server (u32::MAX = default).
    let attr = pulse::def::BufferAttr {
        maxlength: u32::MAX,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: (READ_SAMPLES * std::mem::size_of::<f32>()) as u32,
    };
    Simple::new(
        None,
        APP_NAME,
        pulse::stream::Direction::Record,
        Some(source),
        "Meeting system audio",
        &spec,
        None,
        Some(&attr),
    )
    .map_err(|e| anyhow!("could not open system audio monitor '{source}': {e}"))
}

fn capture_thread(
    mut conn: Introspect,
    mut sink: String,
    mut stream: Simple,
    tx: mpsc::UnboundedSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
) {
    let mut bytes = vec![0u8; READ_SAMPLES * std::mem::size_of::<f32>()];
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = stream.read(&mut bytes) {
            // Read errors end the stream; the capture loop then continues
            // mic-only and tells the user.
            warn!("meeting: system audio read failed: {e}");
            break;
        }
        let samples: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        if tx.unbounded_send(samples).is_err() {
            break;
        }

        // Follow the default output (speakers → headset mid-call).
        if last_check.elapsed() >= DEFAULT_SINK_CHECK {
            last_check = Instant::now();
            match conn.default_sink_name() {
                Ok(Some(current)) if current != sink => {
                    match default_monitor(&mut conn).and_then(|(s, m)| Ok((s, open_record(&m)?))) {
                        Ok((new_sink, new_stream)) => {
                            info!("meeting: default output changed to '{new_sink}'; following it");
                            sink = new_sink;
                            stream = new_stream;
                        }
                        Err(e) => warn!("meeting: could not follow new default output: {e}"),
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    // The query connection died (server restart); try to get a
                    // fresh one but keep recording on the current stream.
                    warn!("meeting: default output check failed: {e}");
                    if let Ok(fresh) = Introspect::connect() {
                        conn = fresh;
                    }
                }
            }
        }
    }
    info!("meeting: system audio capture stopped");
}

/// A stream of mono f32 system-audio samples; dropping it stops the capture.
pub struct SystemAudioStream {
    stop: Arc<AtomicBool>,
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
    pub fn sample_rate(&self) -> u32 {
        SYSTEM_RATE
    }

    /// Live sample rate. Constant on Linux (the server resamples for us).
    pub fn sample_rate_handle(&self) -> Arc<AtomicU32> {
        self.sample_rate_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_route, OutputRoute};

    #[test]
    fn route_prefers_form_factor_then_port_then_bus() {
        assert_eq!(
            classify_route(Some("headset"), Some("analog-output"), Some("usb"), None),
            OutputRoute::Headphones
        );
        assert_eq!(
            classify_route(None, Some("analog-output-speaker"), Some("pci"), None),
            OutputRoute::Speakers
        );
        assert_eq!(
            classify_route(None, Some("analog-output-headphones"), Some("pci"), None),
            OutputRoute::Headphones
        );
        assert_eq!(
            classify_route(None, Some("hdmi-output-1"), Some("pci"), None),
            OutputRoute::Speakers
        );
        assert_eq!(
            classify_route(None, None, Some("bluetooth"), None),
            OutputRoute::Headphones
        );
        assert_eq!(classify_route(None, None, None, None), OutputRoute::Unknown);
    }

    /// Talks to the real sound server: `cargo test --lib pulse::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_server_queries_and_capture() {
        use futures_util::StreamExt;

        for _ in 0..3 {
            assert!(super::sound_server_available());
        }
        println!("route: {:?}", super::detect_output_route());
        println!("recording apps: {:?}", super::recording_apps().unwrap());
        let mut stream = super::SystemAudioCapture::start().unwrap();
        let samples: Vec<f32> = tauri::async_runtime::block_on(async {
            let mut out = Vec::new();
            while out.len() < 48_000 {
                match stream.next().await {
                    Some(s) => out.push(s),
                    None => break,
                }
            }
            out
        });
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("captured {} samples, peak {peak:.4}", samples.len());
        assert_eq!(samples.len(), 48_000);
    }
}
