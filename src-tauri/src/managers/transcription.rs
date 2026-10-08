use crate::audio_toolkit::{apply_custom_words, filter_transcription_output};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::{EngineType, ModelManager};
use crate::settings::{
    get_settings, ModelUnloadTimeout, OrtAcceleratorSetting, WhisperAcceleratorSetting,
};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::Serialize;
use specta::Type;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};
use transcribe_rs::{
    onnx::{
        canary::CanaryModel,
        gigaam::GigaAMModel,
        moonshine::{MoonshineModel, MoonshineVariant, StreamingModel},
        parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity},
        sense_voice::{SenseVoiceModel, SenseVoiceParams},
        Quantization,
    },
    whisper_cpp::{WhisperEngine, WhisperInferenceParams},
    SpeechModel, TranscribeOptions,
};

#[derive(Clone, Debug, Serialize)]
pub struct ModelStateEvent {
    pub event_type: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

enum LoadedEngine {
    Whisper(WhisperEngine),
    Parakeet(ParakeetModel),
    Moonshine(MoonshineModel),
    MoonshineStreaming(StreamingModel),
    SenseVoice(SenseVoiceModel),
    GigaAM(GigaAMModel),
    Canary(CanaryModel),
    /// Cloud transcription via OpenRouter. A lightweight marker: no model is
    /// loaded into memory. The OpenRouter model slug is resolved from the
    /// selected model's `filename` at transcription time, and the actual HTTP
    /// request happens in `transcribe_via_cloud`.
    OpenRouter,
}

/// A short, well-punctuated Turkish style exemplar used as the meeting-mode
/// whisper `initial_prompt`. Priming with correctly cased text that contains the
/// Turkish diacritics (ç ğ ı ö ş ü) nudges whisper toward proper punctuation,
/// casing and diacritics in the output. Kept short so it doesn't crowd out the
/// real audio context.
#[cfg(target_os = "macos")]
const MEETING_TURKISH_STYLE_PROMPT: &str =
    "Merhaba, toplantıya hoş geldiniz. Bugünkü gündem maddelerini gözden geçirelim ve kararları netleştirelim.";

// --- Meeting-mode anti-hallucination inference knobs (macOS meeting path only) ---
// These tune whisper's temperature-fallback decoding to suppress the runaway
// repetition / hallucination that plagues silent or noisy meeting windows.
// Values mirror whisper.cpp's documented defaults except where a stricter
// setting helps meetings specifically. Dictation never sets these (stays None).
//
/// Initial decoding temperature. 0.0 = deterministic/greedy first pass; combined
/// with `MEETING_TEMPERATURE_INC` this enables the temperature fallback loop,
/// the single biggest lever against repetition loops.
#[cfg(target_os = "macos")]
const MEETING_TEMPERATURE: f32 = 0.0;
/// Temperature increment for the fallback loop. When a decode fails the entropy
/// or logprob gate, whisper retries at temperature += this step.
#[cfg(target_os = "macos")]
const MEETING_TEMPERATURE_INC: f32 = 0.2;
/// Entropy (compression-ratio-like) threshold that triggers a temperature-fallback
/// retry. whisper.cpp's documented default.
#[cfg(target_os = "macos")]
const MEETING_ENTROPY_THOLD: f32 = 2.4;
/// Average-logprob threshold that triggers a temperature-fallback retry.
/// whisper.cpp's documented default.
#[cfg(target_os = "macos")]
const MEETING_LOGPROB_THOLD: f32 = -1.0;

/// Options that distinguish the meeting transcription path from dictation.
/// Dictation always uses `dictation()`, which is a no-op (all `None`), so its
/// behavior is byte-for-byte unchanged. Only meeting mode populates these.
#[derive(Clone, Default)]
struct MeetingTranscribeOpts {
    /// Override the transcription language (e.g. "tr" or "auto"). `None` means
    /// use the user's `selected_language` (dictation behavior).
    language_override: Option<String>,
    /// Override whisper's `no_speech_thold`. `None` means use the transcribe-rs
    /// default (dictation behavior).
    no_speech_thold: Option<f32>,
    /// Optional style-exemplar prompt prepended to the whisper `initial_prompt`
    /// (before any custom words). `None` means custom-words-only (dictation).
    style_prompt: Option<&'static str>,
    /// Anti-hallucination temperature-fallback knobs. `None` => transcribe-rs /
    /// whisper.cpp defaults (dictation behavior, unchanged). Meeting mode sets
    /// these so silent/noisy windows fall back instead of looping.
    temperature: Option<f32>,
    temperature_inc: Option<f32>,
    entropy_thold: Option<f32>,
    logprob_thold: Option<f32>,
    /// When `Some(true)`, disable cross-call decoder context. Set only for the
    /// finalize windows, which are chronologically independent slices: carrying
    /// context across them risks propagating a hallucination into later windows.
    /// `None` (live/dictation) preserves default context behavior.
    no_context: Option<bool>,
    /// Which model to transcribe with. `None` means `settings.selected_model`
    /// (dictation). Meeting mode sets this from `settings.meeting_model_id()`
    /// so meetings can run on a different model than push-to-talk dictation.
    ///
    /// This only steers cloud routing, slug lookup and language validation —
    /// for local engines the caller must also have LOADED this model, which is
    /// what `initiate_model_load_for` is for.
    model_override: Option<String>,
}

impl MeetingTranscribeOpts {
    /// Dictation defaults: a no-op so `transcribe()` behaves exactly as before.
    fn dictation() -> Self {
        Self::default()
    }

    /// Meeting-mode options derived from settings.
    ///
    /// `finalize` distinguishes the on-stop / recovery finalize windows (which
    /// transcribe chronologically independent audio slices) from the live rough
    /// pass. Finalize windows additionally set `no_context=true` so a
    /// hallucination in one window can't bleed into the next.
    #[cfg(target_os = "macos")]
    fn meeting(settings: &crate::settings::AppSettings, finalize: bool) -> Self {
        Self {
            language_override: Some(settings.meeting_language.clone()),
            no_speech_thold: Some(0.5),
            style_prompt: Some(MEETING_TURKISH_STYLE_PROMPT),
            temperature: Some(MEETING_TEMPERATURE),
            temperature_inc: Some(MEETING_TEMPERATURE_INC),
            entropy_thold: Some(MEETING_ENTROPY_THOLD),
            logprob_thold: Some(MEETING_LOGPROB_THOLD),
            no_context: if finalize { Some(true) } else { None },
            model_override: Some(settings.meeting_model_id().to_string()),
        }
    }

    /// Build the whisper `initial_prompt`. Dictation: custom words joined, or
    /// `None`. Meeting: style exemplar, then custom words appended.
    fn build_initial_prompt(&self, custom_words: &[String]) -> Option<String> {
        match self.style_prompt {
            Some(style) => {
                if custom_words.is_empty() {
                    Some(style.to_string())
                } else {
                    Some(format!("{} {}", style, custom_words.join(", ")))
                }
            }
            None => {
                if custom_words.is_empty() {
                    None
                } else {
                    Some(custom_words.join(", "))
                }
            }
        }
    }
}

/// Lock a std mutex, recovering the data if a previous holder panicked. Every
/// value guarded in this module stays consistent across a panic (flags, ids,
/// the engine slot), so refusing to continue would only turn one failed
/// transcription into a permanently dead app.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Best-effort text of a caught panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// The transcription failed because no usable engine was resident. Lets the
/// dictation pipeline tell "the model failed to load" apart from "the model
/// ran and failed".
#[derive(Debug)]
pub struct ModelNotLoadedError(pub String);

impl std::fmt::Display for ModelNotLoadedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelNotLoadedError {}

/// RAII guard that clears the `is_loading` flag and notifies waiters on drop.
/// Ensures the loading flag is always reset, even on early returns or panics.
pub struct LoadingGuard {
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        let mut is_loading = lock_or_recover(&self.is_loading);
        *is_loading = false;
        self.loading_condvar.notify_all();
    }
}

/// The resident engine plus the bookkeeping that lets a transcription borrow
/// it without holding the lock for the whole (seconds-long) call.
#[derive(Default)]
struct EngineSlot {
    engine: Option<LoadedEngine>,
    /// Bumped by every load and unload. A transcription that checked the
    /// engine out puts it back only if this is unchanged — otherwise a model
    /// loaded (or unloaded) meanwhile would be silently overwritten by the
    /// stale engine.
    generation: u64,
    /// The engine is out on loan to a running transcription. It is still
    /// loaded as far as everyone else is concerned.
    checked_out: bool,
}

impl EngineSlot {
    fn is_loaded(&self) -> bool {
        self.engine.is_some() || self.checked_out
    }

    /// Replace the resident engine (`None` unloads), invalidating any loan.
    fn install(&mut self, engine: Option<LoadedEngine>) {
        self.engine = engine;
        self.generation = self.generation.wrapping_add(1);
        self.checked_out = false;
    }

    /// Borrow the engine for one transcription. Returns it with the
    /// generation to hand back to [`Self::check_in`].
    fn check_out(&mut self) -> Option<(LoadedEngine, u64)> {
        let engine = self.engine.take()?;
        self.checked_out = true;
        Some((engine, self.generation))
    }

    /// Return a borrowed engine. Dropped instead when the slot was reloaded
    /// or unloaded while it was out. Returns whether it was put back.
    fn check_in(&mut self, engine: LoadedEngine, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.engine = Some(engine);
        self.checked_out = false;
        true
    }

    /// The borrowed engine panicked and is being dropped. Marks the slot
    /// unloaded unless it was already replaced. Returns whether it was.
    fn abandon(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.install(None);
        true
    }
}

#[derive(Clone)]
pub struct TranscriptionManager {
    engine: Arc<Mutex<EngineSlot>>,
    /// Serializes local-engine transcriptions. Without it a second caller
    /// (dictation during a meeting, two finalize windows) would find the slot
    /// empty while the first had the engine checked out and fail with "model
    /// not loaded". Waiters queue instead.
    transcribe_lock: Arc<Mutex<()>>,
    model_manager: Arc<ModelManager>,
    app_handle: AppHandle,
    current_model_id: Arc<Mutex<Option<String>>>,
    /// Why the most recent load failed, for the error shown to the user.
    last_load_error: Arc<Mutex<Option<String>>>,
    last_activity: Arc<AtomicU64>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, model_manager: Arc<ModelManager>) -> Result<Self> {
        let manager = Self {
            engine: Arc::new(Mutex::new(EngineSlot::default())),
            transcribe_lock: Arc::new(Mutex::new(())),
            model_manager,
            app_handle: app_handle.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            last_load_error: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(AtomicU64::new(Self::now_ms())),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
            is_loading: Arc::new(Mutex::new(false)),
            loading_condvar: Arc::new(Condvar::new()),
        };

        // Start the idle watcher
        {
            let app_handle_cloned = app_handle.clone();
            let manager_cloned = manager.clone();
            let shutdown_signal = manager.shutdown_signal.clone();
            let handle = thread::spawn(move || {
                debug!("Idle watcher thread started");
                while !shutdown_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10)); // Check every 10 seconds

                    // Check shutdown signal again after sleep
                    if shutdown_signal.load(Ordering::Relaxed) {
                        break;
                    }

                    let settings = get_settings(&app_handle_cloned);
                    let timeout = settings.model_unload_timeout;

                    // Skip Immediately — that variant is handled by
                    // maybe_unload_immediately() after each transcription.
                    // Treating it as 0s here would unload the model mid-recording.
                    if timeout == ModelUnloadTimeout::Immediately {
                        continue;
                    }

                    // While recording, keep the idle timer fresh so the
                    // model is never unloaded mid-session.
                    let is_recording = app_handle_cloned
                        .try_state::<Arc<AudioRecordingManager>>()
                        .map_or(false, |a| a.is_recording());
                    if is_recording {
                        manager_cloned.touch_activity();
                        continue;
                    }

                    // Meeting mode (Step 3): a meeting session uses an
                    // independent capture path (not the dictation recorder), so
                    // `is_recording()` is false during a meeting. Keep the model
                    // loaded while a meeting is active, otherwise a long quiet
                    // stretch could unload it mid-session. This is purely
                    // additive — it never alters dictation behavior.
                    let meeting_active = app_handle_cloned
                        .try_state::<Arc<crate::meeting::MeetingManager>>()
                        .map_or(false, |m| m.is_active());
                    if meeting_active {
                        manager_cloned.touch_activity();
                        continue;
                    }

                    // Cloud models hold no in-memory engine; never idle-unload
                    // them (it would only flip the UI to "unloaded").
                    if manager_cloned.current_model_is_cloud() {
                        manager_cloned.touch_activity();
                        continue;
                    }

                    if let Some(limit_seconds) = timeout.to_seconds() {
                        let last = manager_cloned.last_activity.load(Ordering::Relaxed);
                        let now_ms = TranscriptionManager::now_ms();
                        let idle_ms = now_ms.saturating_sub(last);
                        let limit_ms = limit_seconds * 1000;

                        if idle_ms > limit_ms {
                            // idle -> unload
                            if manager_cloned.is_model_loaded() {
                                let unload_start = std::time::Instant::now();
                                info!(
                                    "Model idle for {}s (limit: {}s), unloading",
                                    idle_ms / 1000,
                                    limit_seconds
                                );
                                match manager_cloned.unload_model() {
                                    Ok(()) => {
                                        let unload_duration = unload_start.elapsed();
                                        info!(
                                            "Model unloaded due to inactivity (took {}ms)",
                                            unload_duration.as_millis()
                                        );
                                    }
                                    Err(e) => {
                                        error!("Failed to unload idle model: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                debug!("Idle watcher thread shutting down gracefully");
            });
            *lock_or_recover(&manager.watcher_handle) = Some(handle);
        }

        Ok(manager)
    }

    /// Lock the engine slot, recovering from poison if a previous holder panicked.
    fn lock_engine(&self) -> MutexGuard<'_, EngineSlot> {
        lock_or_recover(&self.engine)
    }

    pub fn is_model_loaded(&self) -> bool {
        self.lock_engine().is_loaded()
    }

    /// Atomically check whether a model load is in progress and, if not, mark
    /// one as starting. Returns a [`LoadingGuard`] whose [`Drop`] impl will
    /// clear the flag and wake waiters. Returns `None` if a load is already in
    /// progress.
    pub fn try_start_loading(&self) -> Option<LoadingGuard> {
        let mut is_loading = lock_or_recover(&self.is_loading);
        if *is_loading {
            return None;
        }
        *is_loading = true;
        Some(LoadingGuard {
            is_loading: self.is_loading.clone(),
            loading_condvar: self.loading_condvar.clone(),
        })
    }

    pub fn unload_model(&self) -> Result<()> {
        let unload_start = std::time::Instant::now();
        debug!("Starting to unload model");

        // Dropping the engine frees all resources. A transcription that has
        // it checked out right now drops it when it finishes instead.
        self.lock_engine().install(None);
        {
            let mut current_model = lock_or_recover(&self.current_model_id);
            *current_model = None;
        }

        // Emit unloaded event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "unloaded".to_string(),
                model_id: None,
                model_name: None,
                error: None,
            },
        );

        let unload_duration = unload_start.elapsed();
        debug!(
            "Model unloaded manually (took {}ms)",
            unload_duration.as_millis()
        );
        Ok(())
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Reset the idle timer to now.
    fn touch_activity(&self) {
        self.last_activity.store(Self::now_ms(), Ordering::Relaxed);
    }

    /// Unloads the model immediately if the setting is enabled and the model is loaded
    pub fn maybe_unload_immediately(&self, context: &str) {
        // Cloud models have nothing to free; keep the marker so the UI stays
        // "ready" between dictations.
        if self.current_model_is_cloud() {
            return;
        }
        // The meeting owns the engine; a dictation finishing (or being
        // cancelled) mid-meeting must not pull it out from under it.
        if self.meeting_is_running() {
            return;
        }
        let settings = get_settings(&self.app_handle);
        if settings.model_unload_timeout == ModelUnloadTimeout::Immediately
            && self.is_model_loaded()
        {
            info!("Immediately unloading model after {}", context);
            if let Err(e) = self.unload_model() {
                warn!("Failed to immediately unload model: {}", e);
            }
        }
    }

    /// Load `model_id` as the resident engine.
    ///
    /// Every failure, however early, emits `loading_failed` — the UI's
    /// "loading" spinner is only ever cleared by an event, so a silent early
    /// return would leave it spinning forever.
    pub fn load_model(&self, model_id: &str) -> Result<()> {
        // Emit loading started event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_started".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: None,
                error: None,
            },
        );

        let result = catch_unwind(AssertUnwindSafe(|| self.load_model_inner(model_id)))
            .unwrap_or_else(|panic| {
                Err(anyhow::anyhow!(
                    "Loading model {} panicked: {}",
                    model_id,
                    panic_message(&panic)
                ))
            });

        match &result {
            Ok(()) => *lock_or_recover(&self.last_load_error) = None,
            Err(e) => {
                let error_msg = e.to_string();
                *lock_or_recover(&self.last_load_error) = Some(error_msg.clone());
                let _ = self.app_handle.emit(
                    "model-state-changed",
                    ModelStateEvent {
                        event_type: "loading_failed".to_string(),
                        model_id: Some(model_id.to_string()),
                        model_name: self.model_manager.get_model_info(model_id).map(|m| m.name),
                        error: Some(error_msg),
                    },
                );
            }
        }
        result
    }

    fn load_model_inner(&self, model_id: &str) -> Result<()> {
        let load_start = std::time::Instant::now();
        debug!("Starting to load model: {}", model_id);

        let model_info = self
            .model_manager
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            return Err(anyhow::anyhow!("Model not downloaded"));
        }

        // Cloud models (OpenRouter) have no file to load. Register a lightweight
        // marker engine so the rest of the app (idle unload, model-state events,
        // is_model_loaded) treats them like any loaded model. The API key and
        // request happen later in `transcribe_via_cloud`.
        if model_info.engine_type.is_cloud() {
            self.lock_engine().install(Some(LoadedEngine::OpenRouter));
            *lock_or_recover(&self.current_model_id) = Some(model_id.to_string());
            self.touch_activity();
            let _ = self.app_handle.emit(
                "model-state-changed",
                ModelStateEvent {
                    event_type: "loading_completed".to_string(),
                    model_id: Some(model_id.to_string()),
                    model_name: Some(model_info.name.clone()),
                    error: None,
                },
            );
            debug!(
                "Registered cloud model: {} (took {}ms)",
                model_id,
                load_start.elapsed().as_millis()
            );
            return Ok(());
        }

        let model_path = self.model_manager.get_model_path(model_id)?;

        // Create appropriate engine based on model type. Failures are
        // reported (event + last_load_error) by `load_model`.
        let loaded_engine = match model_info.engine_type {
            EngineType::Whisper => {
                LoadedEngine::Whisper(WhisperEngine::load(&model_path).map_err(|e| {
                    anyhow::anyhow!("Failed to load whisper model {}: {}", model_id, e)
                })?)
            }
            EngineType::Parakeet => LoadedEngine::Parakeet(
                ParakeetModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    anyhow::anyhow!("Failed to load parakeet model {}: {}", model_id, e)
                })?,
            ),
            EngineType::Moonshine => LoadedEngine::Moonshine(
                MoonshineModel::load(
                    &model_path,
                    MoonshineVariant::Base,
                    &Quantization::default(),
                )
                .map_err(|e| {
                    anyhow::anyhow!("Failed to load moonshine model {}: {}", model_id, e)
                })?,
            ),
            EngineType::MoonshineStreaming => LoadedEngine::MoonshineStreaming(
                StreamingModel::load(&model_path, 0, &Quantization::default()).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to load moonshine streaming model {}: {}",
                        model_id,
                        e
                    )
                })?,
            ),
            EngineType::SenseVoice => LoadedEngine::SenseVoice(
                SenseVoiceModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    anyhow::anyhow!("Failed to load SenseVoice model {}: {}", model_id, e)
                })?,
            ),
            EngineType::GigaAM => {
                LoadedEngine::GigaAM(GigaAMModel::load(&model_path, &Quantization::Int8).map_err(
                    |e| anyhow::anyhow!("Failed to load gigaam model {}: {}", model_id, e),
                )?)
            }
            EngineType::Canary => {
                LoadedEngine::Canary(CanaryModel::load(&model_path, &Quantization::Int8).map_err(
                    |e| anyhow::anyhow!("Failed to load canary model {}: {}", model_id, e),
                )?)
            }
            EngineType::OpenRouter
            | EngineType::OpenRouterAsr
            | EngineType::Gemini
            | EngineType::GeminiLive => {
                // Unreachable: cloud models return early above. Kept for match
                // exhaustiveness.
                return Err(anyhow::anyhow!(
                    "internal error: cloud model reached engine dispatch"
                ));
            }
        };

        // Update the current engine and model ID
        self.lock_engine().install(Some(loaded_engine));
        *lock_or_recover(&self.current_model_id) = Some(model_id.to_string());

        // Reset idle timer so the watcher doesn't immediately unload a just-loaded model
        self.touch_activity();

        // Emit loading completed event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_completed".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );

        let load_duration = load_start.elapsed();
        debug!(
            "Successfully loaded transcription model: {} (took {}ms)",
            model_id,
            load_duration.as_millis()
        );
        Ok(())
    }

    /// Background-load the DICTATION model if it is not already resident.
    ///
    /// Skipped entirely while a meeting is running: only one engine is resident
    /// at a time, and the meeting owns it for the duration. Without this a
    /// dictation triggered mid-meeting would swap the meeting's model out and
    /// the next meeting segment would be transcribed by the wrong engine.
    pub fn initiate_model_load(&self) {
        if self.meeting_is_running() {
            debug!("Skipping dictation model load: a meeting owns the engine");
            return;
        }
        let settings = get_settings(&self.app_handle);
        self.initiate_model_load_for(&settings.selected_model);
    }

    /// Whether a meeting session is active (capturing, finalizing or
    /// importing). While it is, the meeting owns the resident engine: nothing
    /// else may load, swap or unload it.
    pub fn meeting_is_running(&self) -> bool {
        self.app_handle
            .try_state::<Arc<crate::meeting::MeetingManager>>()
            .map_or(false, |m| m.is_active())
    }

    /// Background-load a specific model, swapping out whatever is resident if
    /// it is a different one.
    ///
    /// The swap is the point: with dictation and meetings on separate model
    /// settings, "some model is loaded" is no longer the same question as "the
    /// right model is loaded". Returning early on the former would let a
    /// meeting transcribe with the dictation engine while every metadata lookup
    /// (cloud routing, language validation, slug) described the meeting model.
    pub fn initiate_model_load_for(&self, model_id: &str) {
        if self.is_model_loaded() && self.get_current_model().as_deref() == Some(model_id) {
            return;
        }
        // The guard travels into the loader thread and clears the flag when
        // that thread ends, however it ends. A bare flag reset at the end of
        // the closure was skipped by a panic, leaving every later
        // transcription waiting on the condvar forever.
        let Some(guard) = self.try_start_loading() else {
            return;
        };
        let self_clone = self.clone();
        let model_id = model_id.to_string();
        thread::spawn(move || {
            let _guard = guard;
            if let Err(e) = self_clone.load_model(&model_id) {
                error!("Failed to load model: {}", e);
            }
        });
    }

    pub fn get_current_model(&self) -> Option<String> {
        lock_or_recover(&self.current_model_id).clone()
    }

    /// Why the most recent load failed, if it did.
    pub fn last_load_error(&self) -> Option<String> {
        lock_or_recover(&self.last_load_error).clone()
    }

    /// Whether the currently selected model is a cloud (OpenRouter) model.
    /// Cloud models hold no in-memory engine, so they are exempt from idle /
    /// immediate unloading (unloading the marker would only make the UI show
    /// "unloaded" while cloud transcription keeps working).
    fn current_model_is_cloud(&self) -> bool {
        self.get_current_model()
            .and_then(|id| self.model_manager.get_model_info(&id))
            .map_or(false, |m| m.engine_type.is_cloud())
    }

    /// Dictation transcription entry point: the shared `transcribe_with_opts`
    /// with default (dictation) options.
    ///
    /// **Dictation during a meeting** is allowed, but it never swaps the
    /// engine: the meeting owns it. The dictation runs on whatever model the
    /// meeting has resident (and every metadata lookup — cloud routing,
    /// language validation — follows that model, not `selected_model`, so the
    /// two cannot disagree). When nothing usable is resident and the dictation
    /// model is a local one, it fails with a clear message instead.
    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        let mut opts = MeetingTranscribeOpts::dictation();
        if self.meeting_is_running() {
            let selected = get_settings(&self.app_handle).selected_model;
            match self.get_current_model() {
                Some(resident) => {
                    if resident != selected {
                        info!(
                            "Dictation during a meeting: using the meeting's resident model '{}' instead of '{}'",
                            resident, selected
                        );
                    }
                    opts.model_override = Some(resident);
                }
                None => {
                    let selected_is_cloud = self
                        .model_manager
                        .get_model_info(&selected)
                        .is_some_and(|m| m.engine_type.is_cloud());
                    if !selected_is_cloud {
                        return Err(anyhow::Error::new(ModelNotLoadedError(
                            "Dictation is unavailable right now: the running meeting is using the transcription engine.".to_string(),
                        )));
                    }
                }
            }
        }
        self.transcribe_with_opts(audio, opts)
    }

    /// Meeting-mode transcription entry point (ADDITIVE; does not affect
    /// dictation). Forces the configured meeting language, primes punctuation
    /// with a Turkish style exemplar, and raises `no_speech_thold` so silent /
    /// near-silent windows don't hallucinate text. Only the Whisper engine path
    /// honors these knobs; other engines behave exactly as in dictation.
    #[cfg(target_os = "macos")]
    pub fn transcribe_meeting(&self, audio: Vec<f32>) -> Result<String> {
        let settings = get_settings(&self.app_handle);
        self.transcribe_with_opts(audio, MeetingTranscribeOpts::meeting(&settings, false))
    }

    /// Meeting-mode transcription for the on-stop / recovery FINALIZE windows.
    /// Identical to [`transcribe_meeting`] but additionally disables cross-call
    /// decoder context (`no_context`), because finalize windows are
    /// chronologically independent slices and carrying context between them
    /// risks propagating a hallucination forward. ADDITIVE; dictation unaffected.
    #[cfg(target_os = "macos")]
    pub fn transcribe_meeting_finalize(&self, audio: Vec<f32>) -> Result<String> {
        let settings = get_settings(&self.app_handle);
        self.transcribe_with_opts(audio, MeetingTranscribeOpts::meeting(&settings, true))
    }

    /// Cloud (OpenRouter) transcription path. Encodes the audio and sends it to
    /// the configured OpenRouter model, reusing the OpenRouter API key from the
    /// post-processing provider settings (one key for cloud transcription and
    /// post-processing). Returns the raw transcript; custom-word correction and
    /// filler filtering are applied by the caller, same as the local path.
    fn transcribe_via_cloud(
        &self,
        audio: &[f32],
        validated_language: &str,
        settings: &crate::settings::AppSettings,
        slug: &str,
        asr_mode: bool,
    ) -> Result<String> {
        if slug.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "No OpenRouter model selected. Enter a model name in Settings → Models."
            ));
        }

        let mode = if asr_mode {
            crate::managers::cloud_transcription::Mode::Transcription
        } else {
            crate::managers::cloud_transcription::Mode::Chat
        };

        let api_key = settings
            .post_process_api_keys
            .get("openrouter")
            .cloned()
            .unwrap_or_default();

        // Map the app's language code to what we send the model. Whisper's
        // Simplified/Traditional Chinese variants collapse to "zh"; "auto"
        // means let the model detect.
        let language: Option<String> = match validated_language {
            "auto" => None,
            "zh-Hans" | "zh-Hant" => Some("zh".to_string()),
            other => Some(other.to_string()),
        };

        info!("Transcribing via OpenRouter cloud model: {}", slug);

        crate::managers::cloud_transcription::transcribe(
            &api_key,
            slug,
            audio,
            crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE,
            language.as_deref(),
            settings.translate_to_english,
            &settings.custom_words,
            mode,
        )
    }

    /// Cloud transcription straight from Google, for [`EngineType::Gemini`].
    ///
    /// Reuses the batch client the meeting finalize pass uses, with diarization
    /// off: a dictation clip is one speaker by definition, and speaker labels
    /// would only have to be stripped again before pasting. Custom-word
    /// correction is applied by the caller, same as every other path.
    fn transcribe_via_gemini(
        &self,
        audio: &[f32],
        validated_language: &str,
        settings: &crate::settings::AppSettings,
        slug: &str,
    ) -> Result<String> {
        let api_key = settings.gemini_api_key.trim();
        if api_key.is_empty() {
            return Err(anyhow::anyhow!(
                "No Gemini API key set. Add one in Settings → Models."
            ));
        }

        let model = if slug.trim().is_empty() {
            crate::gemini_transcribe::DEFAULT_BATCH_TRANSCRIBE_MODEL.to_string()
        } else {
            slug.trim().to_string()
        };

        // "auto" means send no hint and let the model detect; the Chinese
        // variants collapse to "zh" the same way the other cloud path does.
        let language_codes: Vec<String> = match validated_language {
            "auto" => Vec::new(),
            "zh-Hans" | "zh-Hant" => vec!["zh".to_string()],
            other => vec![other.to_string()],
        };

        let config = crate::gemini_transcribe::BatchTranscribeConfig {
            api_key: api_key.to_string(),
            model,
            language_codes,
            custom_vocabulary: settings.custom_words.clone(),
            // Dictation is inserted verbatim into whatever the user is typing
            // in, so cleanup would silently rewrite their words.
            mode: crate::gemini_transcribe::TranscriptionMode::Verbatim,
            diarize: false,
        };

        info!("Transcribing via Gemini cloud model: {}", config.model);

        let result = crate::gemini_transcribe::transcribe_samples(
            &config,
            audio,
            crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE,
            "dictation",
        )?;

        Ok(result
            .segments
            .into_iter()
            .map(|segment| segment.text)
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string())
    }

    fn transcribe_with_opts(&self, audio: Vec<f32>, opts: MeetingTranscribeOpts) -> Result<String> {
        #[cfg(debug_assertions)]
        if std::env::var("FISILTI_FORCE_TRANSCRIPTION_FAILURE").is_ok() {
            return Err(anyhow::anyhow!(
                "Simulated transcription failure (FISILTI_FORCE_TRANSCRIPTION_FAILURE)"
            ));
        }

        // Update last activity timestamp
        self.touch_activity();

        let st = std::time::Instant::now();

        debug!("Audio vector length: {}", audio.len());

        if audio.is_empty() {
            debug!("Empty audio vector");
            self.maybe_unload_immediately("empty audio");
            return Ok(String::new());
        }

        // Get current settings for configuration
        let settings = get_settings(&self.app_handle);

        // Detect a cloud (OpenRouter) selection up-front. Cloud transcription
        // does not use the local engine machinery at all (no model in memory,
        // no idle-unload race), so it skips the load check below.
        // Dictation reads `selected_model`; meetings pass their own id through
        // `opts`. Everything downstream keys off this one resolved value so the
        // three lookups below can never disagree about which model is running.
        let active_model_id = opts
            .model_override
            .clone()
            .unwrap_or_else(|| settings.selected_model.clone());
        let selected_model_info = self.model_manager.get_model_info(&active_model_id);
        let is_cloud = selected_model_info
            .as_ref()
            .map_or(false, |m| m.engine_type.is_cloud());

        // For local engines, queue behind any transcription already using the
        // engine, then make sure a model is loaded. Held until this call ends.
        let _serial = if is_cloud {
            None
        } else {
            let serial = lock_or_recover(&self.transcribe_lock);

            // If the model is loading, wait for it to complete.
            let mut is_loading = lock_or_recover(&self.is_loading);
            while *is_loading {
                is_loading = self
                    .loading_condvar
                    .wait(is_loading)
                    .unwrap_or_else(|e| e.into_inner());
            }
            drop(is_loading);

            if !self.lock_engine().is_loaded() {
                let reason = self
                    .last_load_error()
                    .unwrap_or_else(|| "no model is loaded".to_string());
                return Err(anyhow::Error::new(ModelNotLoadedError(format!(
                    "The transcription model could not be loaded: {reason}"
                ))));
            }
            Some(serial)
        };

        // Meeting mode can force a specific language (default "tr"); otherwise
        // dictation uses the user's `selected_language`. This keeps dictation's
        // language resolution byte-for-byte identical when `opts` is the
        // dictation default (language_override = None).
        let requested_language = opts
            .language_override
            .clone()
            .unwrap_or_else(|| settings.selected_language.clone());

        // Validate selected language against the model's supported languages.
        // If the language isn't supported, fall back to "auto" to prevent errors.
        let validated_language = if requested_language == "auto" {
            "auto".to_string()
        } else {
            let is_supported = self
                .model_manager
                .get_model_info(&active_model_id)
                .map(|info| {
                    info.supported_languages.is_empty()
                        || info.supported_languages.contains(&requested_language)
                })
                .unwrap_or(true);

            if is_supported {
                requested_language.clone()
            } else {
                warn!(
                    "Language '{}' not supported by current model, falling back to auto-detect",
                    requested_language
                );
                "auto".to_string()
            }
        };

        // Perform transcription with the appropriate engine. Cloud models go
        // out over HTTP; local engines run in-process under catch_unwind to
        // prevent engine panics from poisoning the mutex (which would make the
        // app hang indefinitely on subsequent operations).
        let mut ran_whisper = false;
        let result_text: String = if is_cloud {
            // Preset cloud models carry their OpenRouter slug in `filename`. The
            // "Custom OpenRouter model" entry has an empty filename, so its slug
            // comes from the user-entered `openrouter_custom_model` setting.
            let preset_slug = selected_model_info
                .as_ref()
                .map(|m| m.filename.clone())
                .unwrap_or_default();
            let slug = if preset_slug.trim().is_empty() {
                settings.openrouter_custom_model.trim().to_string()
            } else {
                preset_slug
            };
            // GeminiLive has no batch endpoint. It reaches here only as the
            // fallback when a streaming dictation produced nothing, or when a
            // meeting is pointed at it — both want the batch sibling, not an
            // error, so route them to the same client with the batch model.
            let is_gemini = selected_model_info.as_ref().map_or(false, |m| {
                matches!(m.engine_type, EngineType::Gemini | EngineType::GeminiLive)
            });
            let slug = if selected_model_info
                .as_ref()
                .is_some_and(|m| matches!(m.engine_type, EngineType::GeminiLive))
            {
                crate::gemini_transcribe::DEFAULT_BATCH_TRANSCRIBE_MODEL.to_string()
            } else {
                slug
            };
            if is_gemini {
                // Google direct: different endpoint, different key. Shares the
                // batch client with the meeting finalize pass.
                self.transcribe_via_gemini(&audio, &validated_language, &settings, &slug)?
            } else {
                let asr_mode = selected_model_info.as_ref().map_or(false, |m| {
                    matches!(m.engine_type, EngineType::OpenRouterAsr)
                });
                self.transcribe_via_cloud(&audio, &validated_language, &settings, &slug, asr_mode)?
            }
        } else {
            let result = {
                // Check the engine out so no mutex is held during the engine
                // call. It is returned afterwards unless a load/unload replaced
                // it meanwhile; if the engine panics it is dropped (effectively
                // unloading it) instead of poisoning the mutex.
                let (mut engine, generation) = match self.lock_engine().check_out() {
                    Some(loan) => loan,
                    None => {
                        return Err(anyhow::Error::new(ModelNotLoadedError(
                            "The model was unloaded before transcription could start.".to_string(),
                        )));
                    }
                };

                let transcribe_result = catch_unwind(AssertUnwindSafe(
                    || -> Result<transcribe_rs::TranscriptionResult> {
                        match &mut engine {
                            LoadedEngine::Whisper(whisper_engine) => {
                                let whisper_language = if validated_language == "auto" {
                                    None
                                } else {
                                    let normalized = if validated_language == "zh-Hans"
                                        || validated_language == "zh-Hant"
                                    {
                                        "zh".to_string()
                                    } else {
                                        validated_language.clone()
                                    };
                                    Some(normalized)
                                };

                                // Initial prompt: dictation primes only with custom
                                // words (unchanged). Meeting mode prepends a
                                // well-punctuated Turkish style exemplar so whisper
                                // emits proper casing + diacritics, then appends any
                                // custom words.
                                let initial_prompt =
                                    opts.build_initial_prompt(&settings.custom_words);

                                let params = WhisperInferenceParams {
                                    language: whisper_language,
                                    translate: settings.translate_to_english,
                                    initial_prompt,
                                    // Meeting mode raises this above the 0.2 default
                                    // to drop silent windows that would otherwise
                                    // hallucinate. Dictation keeps the default.
                                    no_speech_thold: opts.no_speech_thold.unwrap_or(
                                        WhisperInferenceParams::default().no_speech_thold,
                                    ),
                                    // Anti-hallucination knobs (meeting only). All
                                    // `None` for dictation, so its params are
                                    // byte-for-byte identical to before.
                                    temperature: opts.temperature,
                                    temperature_inc: opts.temperature_inc,
                                    entropy_thold: opts.entropy_thold,
                                    logprob_thold: opts.logprob_thold,
                                    no_context: opts.no_context,
                                    ..Default::default()
                                };

                                whisper_engine
                                    .transcribe_with(&audio, &params)
                                    .map_err(|e| {
                                        anyhow::anyhow!("Whisper transcription failed: {}", e)
                                    })
                            }
                            LoadedEngine::Parakeet(parakeet_engine) => {
                                let params = ParakeetParams {
                                    timestamp_granularity: Some(TimestampGranularity::Segment),
                                    ..Default::default()
                                };
                                parakeet_engine
                                    .transcribe_with(&audio, &params)
                                    .map_err(|e| {
                                        anyhow::anyhow!("Parakeet transcription failed: {}", e)
                                    })
                            }
                            LoadedEngine::Moonshine(moonshine_engine) => moonshine_engine
                                .transcribe(&audio, &TranscribeOptions::default())
                                .map_err(|e| {
                                    anyhow::anyhow!("Moonshine transcription failed: {}", e)
                                }),
                            LoadedEngine::MoonshineStreaming(streaming_engine) => streaming_engine
                                .transcribe(&audio, &TranscribeOptions::default())
                                .map_err(|e| {
                                    anyhow::anyhow!(
                                        "Moonshine streaming transcription failed: {}",
                                        e
                                    )
                                }),
                            LoadedEngine::SenseVoice(sense_voice_engine) => {
                                let language = match validated_language.as_str() {
                                    "zh" | "zh-Hans" | "zh-Hant" => Some("zh".to_string()),
                                    "en" => Some("en".to_string()),
                                    "ja" => Some("ja".to_string()),
                                    "ko" => Some("ko".to_string()),
                                    "yue" => Some("yue".to_string()),
                                    _ => None,
                                };
                                let params = SenseVoiceParams {
                                    language,
                                    use_itn: Some(true),
                                };
                                sense_voice_engine
                                    .transcribe_with(&audio, &params)
                                    .map_err(|e| {
                                        anyhow::anyhow!("SenseVoice transcription failed: {}", e)
                                    })
                            }
                            LoadedEngine::GigaAM(gigaam_engine) => gigaam_engine
                                .transcribe(&audio, &TranscribeOptions::default())
                                .map_err(|e| anyhow::anyhow!("GigaAM transcription failed: {}", e)),
                            LoadedEngine::Canary(canary_engine) => {
                                let lang = if validated_language == "auto" {
                                    None
                                } else {
                                    Some(validated_language.clone())
                                };
                                let options = TranscribeOptions {
                                    language: lang,
                                    translate: settings.translate_to_english,
                                };
                                canary_engine.transcribe(&audio, &options).map_err(|e| {
                                    anyhow::anyhow!("Canary transcription failed: {}", e)
                                })
                            }
                            LoadedEngine::OpenRouter => Err(anyhow::anyhow!(
                                "internal error: cloud model reached local transcription dispatch"
                            )),
                        }
                    },
                ));

                ran_whisper = matches!(engine, LoadedEngine::Whisper(_));

                match transcribe_result {
                    Ok(inner_result) => {
                        // Success or normal error — put the engine back, unless
                        // a load/unload replaced it while it was out.
                        if !self.lock_engine().check_in(engine, generation) {
                            debug!(
                                "Engine was replaced during transcription; dropping the old one"
                            );
                        }
                        inner_result?
                    }
                    Err(panic_payload) => {
                        // Engine panicked — do NOT put it back (it's in an unknown state).
                        // The engine is dropped here, effectively unloading it.
                        drop(engine);
                        let panic_msg = panic_message(&panic_payload);
                        error!(
                            "Transcription engine panicked: {}. Model has been unloaded.",
                            panic_msg
                        );

                        // Mark the slot unloaded and clear the model ID so it
                        // reloads on the next attempt — unless a newer model
                        // was installed meanwhile, which must be left alone.
                        if self.lock_engine().abandon(generation) {
                            *lock_or_recover(&self.current_model_id) = None;
                        }

                        let _ = self.app_handle.emit(
                            "model-state-changed",
                            ModelStateEvent {
                                event_type: "unloaded".to_string(),
                                model_id: None,
                                model_name: None,
                                error: Some(format!("Engine panicked: {}", panic_msg)),
                            },
                        );

                        return Err(anyhow::anyhow!(
                        "Transcription engine panicked: {}. The model has been unloaded and will reload on next attempt.",
                        panic_msg
                    ));
                    }
                }
            };
            result.text
        };

        // Apply word correction if custom words are configured. Skip when the
        // engine that actually ran was Whisper, since custom words were already
        // passed as its initial_prompt. (Reading `selected_model` here was
        // wrong for meetings and for dictation during a meeting.)
        let corrected_result = if !settings.custom_words.is_empty() && !ran_whisper {
            apply_custom_words(
                &result_text,
                &settings.custom_words,
                settings.word_correction_threshold,
            )
        } else {
            result_text
        };

        // Filter out filler words and hallucinations
        let filtered_result = filter_transcription_output(
            &corrected_result,
            &settings.app_language,
            &settings.custom_filler_words,
        );

        let et = std::time::Instant::now();
        let translation_note = if settings.translate_to_english {
            " (translated)"
        } else {
            ""
        };
        info!(
            "Transcription completed in {}ms{}",
            (et - st).as_millis(),
            translation_note
        );

        let final_result = filtered_result;

        if final_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            info!("Transcription result: {}", final_result);
        }

        self.maybe_unload_immediately("transcription");

        Ok(final_result)
    }
}

/// Apply the user's accelerator preferences to the transcribe-rs global atomics.
/// Called on startup and whenever the user changes the setting.
pub fn apply_accelerator_settings(app: &tauri::AppHandle) {
    use transcribe_rs::accel;

    let settings = get_settings(app);

    let whisper_pref = match settings.whisper_accelerator {
        WhisperAcceleratorSetting::Auto => accel::WhisperAccelerator::Auto,
        WhisperAcceleratorSetting::Cpu => accel::WhisperAccelerator::CpuOnly,
        WhisperAcceleratorSetting::Gpu => accel::WhisperAccelerator::Gpu,
    };
    accel::set_whisper_accelerator(whisper_pref);
    info!("Whisper accelerator set to: {}", whisper_pref);

    let ort_pref = match settings.ort_accelerator {
        OrtAcceleratorSetting::Auto => accel::OrtAccelerator::Auto,
        OrtAcceleratorSetting::Cpu => accel::OrtAccelerator::CpuOnly,
        OrtAcceleratorSetting::Cuda => accel::OrtAccelerator::Cuda,
        OrtAcceleratorSetting::DirectMl => accel::OrtAccelerator::DirectMl,
        OrtAcceleratorSetting::Rocm => accel::OrtAccelerator::Rocm,
    };
    accel::set_ort_accelerator(ort_pref);
    info!("ORT accelerator set to: {}", ort_pref);
}

#[derive(Serialize, Clone, Debug, Type)]
pub struct AvailableAccelerators {
    pub whisper: Vec<String>,
    pub ort: Vec<String>,
}

/// Return which accelerators are compiled into this build.
pub fn get_available_accelerators() -> AvailableAccelerators {
    use transcribe_rs::accel::OrtAccelerator;

    let ort_options: Vec<String> = OrtAccelerator::available()
        .into_iter()
        .map(|a| a.to_string())
        .collect();

    let whisper_options = vec!["auto".to_string(), "cpu".to_string(), "gpu".to_string()];

    AvailableAccelerators {
        whisper: whisper_options,
        ort: ort_options,
    }
}

impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        // Skip shutdown unless this is the very last clone. TranscriptionManager
        // is cloned by initiate_model_load() and the watcher thread — those
        // clones dropping must not kill the watcher. The watcher thread holds
        // its own clone, so engine's strong_count is always >= 2 while the
        // watcher is alive. When it reaches 1, only this instance remains
        // and we can safely shut down.
        if Arc::strong_count(&self.engine) > 1 {
            return;
        }

        // Signal the watcher thread to shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for the thread to finish gracefully
        if let Some(handle) = lock_or_recover(&self.watcher_handle).take() {
            if let Err(e) = handle.join() {
                warn!("Failed to join idle watcher thread: {:?}", e);
            } else {
                debug!("Idle watcher thread joined successfully");
            }
        }
    }
}

#[cfg(test)]
mod engine_slot_tests {
    use super::*;

    fn marker() -> LoadedEngine {
        LoadedEngine::OpenRouter
    }

    #[test]
    fn a_checked_out_engine_still_counts_as_loaded() {
        let mut slot = EngineSlot::default();
        slot.install(Some(marker()));
        let (engine, gen) = slot.check_out().unwrap();
        assert!(slot.is_loaded());
        assert!(slot.check_in(engine, gen));
        assert!(slot.is_loaded());
        assert!(slot.engine.is_some());
    }

    #[test]
    fn a_load_during_transcription_is_not_overwritten_by_the_stale_engine() {
        let mut slot = EngineSlot::default();
        slot.install(Some(marker()));
        let (old, gen) = slot.check_out().unwrap();

        // A model switch lands while the old engine is out on loan.
        slot.install(Some(marker()));
        let new_gen = slot.generation;

        assert!(!slot.check_in(old, gen));
        assert_eq!(slot.generation, new_gen);
        assert!(slot.engine.is_some());
    }

    #[test]
    fn an_unload_during_transcription_stays_unloaded() {
        let mut slot = EngineSlot::default();
        slot.install(Some(marker()));
        let (old, gen) = slot.check_out().unwrap();
        slot.install(None);
        assert!(!slot.is_loaded());
        assert!(!slot.check_in(old, gen));
        assert!(!slot.is_loaded());
    }

    #[test]
    fn a_panicked_engine_unloads_only_its_own_generation() {
        let mut slot = EngineSlot::default();
        slot.install(Some(marker()));
        let (_, gen) = slot.check_out().unwrap();
        assert!(slot.abandon(gen));
        assert!(!slot.is_loaded());

        slot.install(Some(marker()));
        let (_, gen) = slot.check_out().unwrap();
        slot.install(Some(marker()));
        assert!(!slot.abandon(gen), "a newer engine must survive");
        assert!(slot.is_loaded());
    }

    #[test]
    fn nothing_to_check_out_when_unloaded() {
        let mut slot = EngineSlot::default();
        assert!(slot.check_out().is_none());
        assert!(!slot.is_loaded());
    }
}
