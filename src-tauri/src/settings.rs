use log::{debug, warn};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;
use std::collections::HashMap;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

pub const APPLE_INTELLIGENCE_PROVIDER_ID: &str = "apple_intelligence";
pub const APPLE_INTELLIGENCE_DEFAULT_MODEL_ID: &str = "Apple Intelligence";

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// Custom deserializer to handle both old numeric format (1-5) and new string format ("trace", "debug", etc.)
impl<'de> Deserialize<'de> for LogLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LogLevelVisitor;

        impl<'de> Visitor<'de> for LogLevelVisitor {
            type Value = LogLevel;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or integer representing log level")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<LogLevel, E> {
                match value.to_lowercase().as_str() {
                    "trace" => Ok(LogLevel::Trace),
                    "debug" => Ok(LogLevel::Debug),
                    "info" => Ok(LogLevel::Info),
                    "warn" => Ok(LogLevel::Warn),
                    "error" => Ok(LogLevel::Error),
                    _ => Err(E::unknown_variant(
                        value,
                        &["trace", "debug", "info", "warn", "error"],
                    )),
                }
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<LogLevel, E> {
                match value {
                    1 => Ok(LogLevel::Trace),
                    2 => Ok(LogLevel::Debug),
                    3 => Ok(LogLevel::Info),
                    4 => Ok(LogLevel::Warn),
                    5 => Ok(LogLevel::Error),
                    _ => Err(E::invalid_value(de::Unexpected::Unsigned(value), &"1-5")),
                }
            }
        }

        deserializer.deserialize_any(LogLevelVisitor)
    }
}

impl From<LogLevel> for tauri_plugin_log::LogLevel {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => tauri_plugin_log::LogLevel::Trace,
            LogLevel::Debug => tauri_plugin_log::LogLevel::Debug,
            LogLevel::Info => tauri_plugin_log::LogLevel::Info,
            LogLevel::Warn => tauri_plugin_log::LogLevel::Warn,
            LogLevel::Error => tauri_plugin_log::LogLevel::Error,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_binding: String,
    pub current_binding: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct LLMPrompt {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct PostProcessProvider {
    pub id: String,
    pub label: String,
    pub base_url: String,
    #[serde(default)]
    pub allow_base_url_edit: bool,
    #[serde(default)]
    pub models_endpoint: Option<String>,
    #[serde(default)]
    pub supports_structured_output: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    None,
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelUnloadTimeout {
    Never,
    Immediately,
    Min2,
    Min5,
    Min10,
    Min15,
    Hour1,
    Sec15, // Debug mode only
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PasteMethod {
    CtrlV,
    Direct,
    None,
    ShiftInsert,
    CtrlShiftV,
    ExternalScript,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardHandling {
    DontModify,
    CopyToClipboard,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum AutoSubmitKey {
    Enter,
    CtrlEnter,
    CmdEnter,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum RecordingRetentionPeriod {
    Never,
    PreserveLimit,
    Days3,
    Weeks2,
    Months3,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardImplementation {
    Tauri,
    HandyKeys,
}

impl Default for KeyboardImplementation {
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        return KeyboardImplementation::Tauri;
        #[cfg(not(target_os = "linux"))]
        return KeyboardImplementation::HandyKeys;
    }
}

impl Default for ModelUnloadTimeout {
    fn default() -> Self {
        ModelUnloadTimeout::Min5
    }
}

impl Default for PasteMethod {
    fn default() -> Self {
        // Default to CtrlV for macOS and Windows, Direct for Linux
        #[cfg(target_os = "linux")]
        return PasteMethod::Direct;
        #[cfg(not(target_os = "linux"))]
        return PasteMethod::CtrlV;
    }
}

impl Default for ClipboardHandling {
    fn default() -> Self {
        ClipboardHandling::DontModify
    }
}

impl Default for AutoSubmitKey {
    fn default() -> Self {
        AutoSubmitKey::Enter
    }
}

impl ModelUnloadTimeout {
    pub fn to_minutes(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0), // Special case for immediate unloading
            ModelUnloadTimeout::Min2 => Some(2),
            ModelUnloadTimeout::Min5 => Some(5),
            ModelUnloadTimeout::Min10 => Some(10),
            ModelUnloadTimeout::Min15 => Some(15),
            ModelUnloadTimeout::Hour1 => Some(60),
            ModelUnloadTimeout::Sec15 => Some(0), // Special case for debug - handled separately
        }
    }

    pub fn to_seconds(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0), // Special case for immediate unloading
            ModelUnloadTimeout::Sec15 => Some(15),
            _ => self.to_minutes().map(|m| m * 60),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SoundTheme {
    Marimba,
    Pop,
    Custom,
}

impl SoundTheme {
    fn as_str(&self) -> &'static str {
        match self {
            SoundTheme::Marimba => "marimba",
            SoundTheme::Pop => "pop",
            SoundTheme::Custom => "custom",
        }
    }

    pub fn to_start_path(&self) -> String {
        format!("resources/{}_start.wav", self.as_str())
    }

    pub fn to_stop_path(&self) -> String {
        format!("resources/{}_stop.wav", self.as_str())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum TypingTool {
    Auto,
    Wtype,
    Kwtype,
    Dotool,
    Ydotool,
    Xdotool,
}

impl Default for TypingTool {
    fn default() -> Self {
        TypingTool::Auto
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum WhisperAcceleratorSetting {
    Auto,
    Cpu,
    Gpu,
}

impl Default for WhisperAcceleratorSetting {
    fn default() -> Self {
        WhisperAcceleratorSetting::Auto
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum OrtAcceleratorSetting {
    Auto,
    Cpu,
    Cuda,
    #[serde(rename = "directml")]
    DirectMl,
    Rocm,
}

impl Default for OrtAcceleratorSetting {
    fn default() -> Self {
        OrtAcceleratorSetting::Auto
    }
}

/* still handy for composing the initial JSON in the store ------------- */
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AppSettings {
    pub bindings: HashMap<String, ShortcutBinding>,
    pub push_to_talk: bool,
    pub audio_feedback: bool,
    #[serde(default = "default_audio_feedback_volume")]
    pub audio_feedback_volume: f32,
    #[serde(default = "default_sound_theme")]
    pub sound_theme: SoundTheme,
    #[serde(default = "default_start_hidden")]
    pub start_hidden: bool,
    #[serde(default = "default_autostart_enabled")]
    pub autostart_enabled: bool,
    #[serde(default = "default_update_checks_enabled")]
    pub update_checks_enabled: bool,
    #[serde(default = "default_model")]
    pub selected_model: String,
    #[serde(default = "default_always_on_microphone")]
    pub always_on_microphone: bool,
    #[serde(default)]
    pub selected_microphone: Option<String>,
    #[serde(default)]
    pub clamshell_microphone: Option<String>,
    #[serde(default)]
    pub selected_output_device: Option<String>,
    #[serde(default = "default_translate_to_english")]
    pub translate_to_english: bool,
    #[serde(default = "default_selected_language")]
    pub selected_language: String,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: OverlayPosition,
    #[serde(default = "default_debug_mode")]
    pub debug_mode: bool,
    #[serde(default = "default_log_level")]
    pub log_level: LogLevel,
    #[serde(default)]
    pub custom_words: Vec<String>,
    #[serde(default)]
    pub model_unload_timeout: ModelUnloadTimeout,
    #[serde(default = "default_word_correction_threshold")]
    pub word_correction_threshold: f64,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default = "default_recording_retention_period")]
    pub recording_retention_period: RecordingRetentionPeriod,
    #[serde(default)]
    pub paste_method: PasteMethod,
    #[serde(default)]
    pub clipboard_handling: ClipboardHandling,
    #[serde(default = "default_auto_submit")]
    pub auto_submit: bool,
    #[serde(default)]
    pub auto_submit_key: AutoSubmitKey,
    #[serde(default = "default_post_process_enabled")]
    pub post_process_enabled: bool,
    #[serde(default = "default_post_process_provider_id")]
    pub post_process_provider_id: String,
    #[serde(default = "default_post_process_providers")]
    pub post_process_providers: Vec<PostProcessProvider>,
    #[serde(default = "default_post_process_api_keys")]
    pub post_process_api_keys: HashMap<String, String>,
    #[serde(default = "default_post_process_models")]
    pub post_process_models: HashMap<String, String>,
    #[serde(default = "default_post_process_prompts")]
    pub post_process_prompts: Vec<LLMPrompt>,
    #[serde(default)]
    pub post_process_selected_prompt_id: Option<String>,
    /// Custom OpenRouter model slug used by the "Custom OpenRouter model" cloud
    /// transcription entry (e.g. `openai/gpt-4o-mini-transcribe`). Empty unless
    /// the user enters one in Settings → Models.
    #[serde(default)]
    pub openrouter_custom_model: String,
    #[serde(default)]
    pub mute_while_recording: bool,
    #[serde(default)]
    pub append_trailing_space: bool,
    #[serde(default = "default_app_language")]
    pub app_language: String,
    #[serde(default)]
    pub experimental_enabled: bool,
    #[serde(default)]
    pub lazy_stream_close: bool,
    #[serde(default)]
    pub keyboard_implementation: KeyboardImplementation,
    #[serde(default = "default_show_tray_icon")]
    pub show_tray_icon: bool,
    #[serde(default = "default_paste_delay_ms")]
    pub paste_delay_ms: u64,
    #[serde(default = "default_typing_tool")]
    pub typing_tool: TypingTool,
    pub external_script_path: Option<String>,
    #[serde(default)]
    pub custom_filler_words: Option<Vec<String>>,
    #[serde(default)]
    pub whisper_accelerator: WhisperAcceleratorSetting,
    #[serde(default)]
    pub ort_accelerator: OrtAcceleratorSetting,
    #[serde(default)]
    pub extra_recording_buffer_ms: u64,
    /// Meeting mode: automatically summarize the meeting transcript on stop
    /// (using the active post-process provider). Additive; defaults to false.
    #[serde(default)]
    pub meeting_auto_summarize: bool,
    /// Meeting mode: which model transcribes meetings, independent of the model
    /// dictation uses.
    ///
    /// Empty (the default) means "whatever dictation uses", which is both the
    /// old behaviour and a sensible default — it keeps following
    /// [`Self::selected_model`] when the user changes that. Set it to run
    /// meetings on something else: an accurate cloud model for meetings while
    /// push-to-talk dictation stays local and free.
    ///
    /// Read through [`Self::meeting_model_id`], never directly.
    #[serde(default)]
    pub meeting_selected_model: String,
    /// Meeting mode: the model used for the high-quality FINAL (on-stop)
    /// re-transcription pass. Swapped in for finalize only, then the meeting
    /// model is restored. Defaults to "turbo" (large-v3-turbo). The LIVE
    /// preview path uses [`Self::meeting_model_id`].
    #[serde(default = "default_meeting_final_model")]
    pub meeting_final_model: String,
    /// Meeting mode: language forced for meeting transcription windows. Fixes
    /// per-window language flapping that auto-detect causes. Defaults to "tr"
    /// (Turkish). Set to "auto" to restore per-window auto-detect. Only affects
    /// the meeting path; dictation keeps using `selected_language`.
    #[serde(default = "default_meeting_language")]
    pub meeting_language: String,
    /// Meeting mode: preset summary prompt templates the user can choose from
    /// when summarizing a meeting. Each produces structured markdown. Additive;
    /// defaults to a built-in set (General / Standup / 1:1 / Decisions & Action
    /// Items / Q&A). The picker UI is Phase 4; the backend exposes them now.
    #[serde(default = "default_meeting_summary_templates")]
    pub meeting_summary_templates: Vec<MeetingSummaryTemplate>,
    /// Meeting mode: automatically detect when a meeting app (Zoom, Teams, a
    /// browser running Meet, …) starts using the microphone and show a prompt
    /// offering to start a transcription session. macOS only. Opt-in.
    #[serde(default)]
    pub meeting_auto_detect: bool,
    /// Meeting mode: while a session is running, offer to end it after
    /// `meeting_silence_timeout_secs` of continuous silence (or when the
    /// detected meeting app releases the microphone), and end it automatically
    /// if the prompt goes unanswered for `meeting_auto_end_grace_secs`.
    #[serde(default = "default_meeting_auto_end")]
    pub meeting_auto_end: bool,
    /// Seconds of continuous silence before the "end meeting?" prompt appears.
    #[serde(default = "default_meeting_silence_timeout_secs")]
    pub meeting_silence_timeout_secs: u32,
    /// Meeting mode: name new sessions after the calendar event in progress at
    /// start time (macOS EventKit). Opt-in: enabling prompts for the Calendars
    /// permission. Window-title naming and the LLM auto-title stay available
    /// regardless.
    #[serde(default)]
    pub meeting_calendar_names: bool,
    /// Meeting mode: folder every completed meeting is also written to as a
    /// Markdown file (an Obsidian vault, a notes repo). Empty = off. The file is
    /// rewritten when the meeting's title, summary or notes change.
    #[serde(default)]
    pub meeting_export_dir: String,
    /// Seconds the "end meeting?" prompt waits for a response before the
    /// session is ended automatically.
    #[serde(default = "default_meeting_auto_end_grace_secs")]
    pub meeting_auto_end_grace_secs: u32,
    /// Meeting mode: stream the meeting audio to the Gemini Live API instead of
    /// waiting for the on-stop finalize pass. One of `"off"`, `"translate"`
    /// (speech-to-speech translation, reports original + translation) or
    /// `"transcribe"` (transcription only, several times cheaper). Opt-in,
    /// needs `gemini_api_key`. macOS only (it rides the meeting capture loop).
    #[serde(default = "default_meeting_live_mode")]
    pub meeting_live_mode: String,
    /// BCP-47 code live translation translates INTO (e.g. "en", "tr", "de").
    #[serde(default = "default_meeting_live_translate_target")]
    pub meeting_live_translate_target: String,
    /// Live API model id used for live translation. Overridable so a newer
    /// preview can be selected without a rebuild.
    #[serde(default = "default_meeting_live_translate_model")]
    pub meeting_live_translate_model: String,
    /// Live API model id used for live transcription (no translation).
    #[serde(default = "default_meeting_live_transcribe_model")]
    pub meeting_live_transcribe_model: String,
    /// DEPRECATED, kept only so [`migrate_gemini_finalize_to_meeting_model`]
    /// can read it off existing installs.
    ///
    /// This used to be a toggle: "run the on-stop finalize through Gemini
    /// instead of the local Whisper windows". Choosing Gemini as the meeting
    /// model now says the same thing in one place, and having both meant two
    /// Gemini paths of different quality selected by a hidden boolean. Nothing
    /// reads this field any more; the migration clears it.
    #[serde(default)]
    pub meeting_gemini_finalize: bool,
    /// Batch transcription model id for the Gemini finalize pass.
    #[serde(default = "default_meeting_gemini_finalize_model")]
    pub meeting_gemini_finalize_model: String,
    /// Ask the batch finalize pass to attribute speech to distinct speakers.
    /// Automatically skipped for audio past the API's 30-minute diarization
    /// limit.
    #[serde(default = "default_true")]
    pub meeting_gemini_diarize: bool,
    /// Ask Gemini to clean disfluencies and format the transcript ("smart"
    /// mode) rather than transcribe verbatim. Applies to both the batch
    /// finalize pass and live transcription.
    #[serde(default = "default_true")]
    pub meeting_gemini_smart: bool,
    /// Show a click-through subtitle strip near the bottom of the screen while
    /// a Gemini Live stream is running. Defaults ON because it only ever
    /// appears once a live mode is already explicitly enabled — the text is the
    /// thing that mode was turned on to produce, and the app window is not
    /// where anyone is looking during a call.
    #[serde(default = "default_true")]
    pub meeting_subtitles: bool,
    /// Domain terms, names and product names Gemini should prefer, one per line
    /// (commas also accepted). Used by both Gemini transcription paths.
    #[serde(default)]
    /// DEPRECATED, kept only so [`migrate_meeting_vocabulary_into_custom_words`]
    /// can read it off existing installs.
    ///
    /// Meeting transcription used to keep its own term list while dictation had
    /// [`Self::custom_words`], so the same name had to be typed twice — and it
    /// was never obvious which list a term belonged in. Both now read
    /// `custom_words`; the migration folds this one into it.
    pub meeting_custom_vocabulary: String,
    /// API key for Google's Gemini API, used by every Gemini meeting path.
    /// Separate from `post_process_api_keys` because that map is keyed by
    /// post-processing provider, and this is not one.
    #[serde(default)]
    pub gemini_api_key: String,
}

fn default_meeting_live_mode() -> String {
    "off".to_string()
}

fn default_meeting_live_translate_target() -> String {
    "en".to_string()
}

fn default_meeting_live_translate_model() -> String {
    crate::gemini_live::DEFAULT_LIVE_TRANSLATE_MODEL.to_string()
}

fn default_meeting_live_transcribe_model() -> String {
    crate::gemini_live::DEFAULT_LIVE_TRANSCRIBE_MODEL.to_string()
}

fn default_meeting_gemini_finalize_model() -> String {
    crate::gemini_transcribe::DEFAULT_BATCH_TRANSCRIBE_MODEL.to_string()
}

fn default_true() -> bool {
    true
}

fn default_meeting_auto_end() -> bool {
    true
}

fn default_meeting_silence_timeout_secs() -> u32 {
    180
}

fn default_meeting_auto_end_grace_secs() -> u32 {
    60
}

/// A preset summary prompt template for meeting mode. `id` is a stable key the
/// frontend passes to `summarize_meeting_with`; `name` is the display label;
/// `prompt` is the system instruction sent to the LLM (it should instruct the
/// model to produce structured markdown and to answer in the transcript's
/// language).
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct MeetingSummaryTemplate {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

fn default_model() -> String {
    "".to_string()
}

fn default_meeting_final_model() -> String {
    "turbo".to_string()
}

fn default_meeting_language() -> String {
    "tr".to_string()
}

/// Built-in meeting summary templates. Each prompt instructs the model to emit
/// structured markdown and to answer in the SAME language as the transcript.
/// The frontend picker (Phase 4) lets the user choose one by `id`.
pub fn default_meeting_summary_templates() -> Vec<MeetingSummaryTemplate> {
    let common = "Respond in the SAME LANGUAGE as the transcript (do not translate). \
Format the answer as clean, well-structured Markdown. Use the section headings in the \
transcript's language. Only use information present in the transcript; do not invent details.";
    vec![
        MeetingSummaryTemplate {
            id: "general".to_string(),
            name: "General".to_string(),
            prompt: format!(
                "You write clear, concise meeting notes from a raw transcript. {common}\n\n\
Produce these Markdown sections:\n\
## Summary - a short paragraph.\n\
## Key discussion points - a bullet list of the main topics.\n\
## Decisions - a bullet list of decisions made (or note none).\n\
## Action items - a bullet list of follow-ups, with the responsible person if mentioned."
            ),
        },
        MeetingSummaryTemplate {
            id: "standup".to_string(),
            name: "Standup".to_string(),
            prompt: format!(
                "You summarize a team standup from a raw transcript. {common}\n\n\
Produce a Markdown section per participant (use their name as a `###` heading), each with:\n\
- **Yesterday / Done**: what they completed.\n\
- **Today / Next**: what they will work on.\n\
- **Blockers**: anything blocking them (or none).\n\
End with a `## Blockers & follow-ups` section aggregating open blockers."
            ),
        },
        MeetingSummaryTemplate {
            id: "one_on_one".to_string(),
            name: "1:1".to_string(),
            prompt: format!(
                "You write notes for a 1:1 meeting from a raw transcript. {common}\n\n\
Produce these Markdown sections:\n\
## Topics discussed - a bullet list.\n\
## Feedback - feedback exchanged in either direction.\n\
## Decisions & agreements - what was agreed.\n\
## Action items - follow-ups with owner and (if mentioned) due date."
            ),
        },
        MeetingSummaryTemplate {
            id: "decisions_actions".to_string(),
            name: "Decisions & Action Items".to_string(),
            prompt: format!(
                "You extract the decisions and action items from a raw meeting transcript. {common}\n\n\
Produce these Markdown sections:\n\
## Decisions - a numbered list of every decision made, each with a one-line rationale if stated.\n\
## Action items - a table with columns | Task | Owner | Due | so each row is one follow-up. \
Use '-' where a field is unknown.\n\
Keep it focused on decisions and actions; omit general discussion."
            ),
        },
        MeetingSummaryTemplate {
            id: "qa".to_string(),
            name: "Q&A".to_string(),
            prompt: format!(
                "You summarize a Q&A / interview session from a raw transcript. {common}\n\n\
Produce a `## Q&A` Markdown section listing each question and its answer as:\n\
**Q:** <the question>\n\n**A:** <the answer, concise>\n\n\
Preserve the order questions were asked. End with a `## Open questions` section for any \
questions left unanswered."
            ),
        },
    ]
}

fn default_always_on_microphone() -> bool {
    false
}

fn default_translate_to_english() -> bool {
    false
}

fn default_start_hidden() -> bool {
    false
}

fn default_autostart_enabled() -> bool {
    false
}

fn default_update_checks_enabled() -> bool {
    // Fısıltı has no update server yet. Default OFF so the app doesn't check
    // (the inherited updater endpoint pointed at the upstream project's releases, which
    // produced a false "update available"). Enable once our own release feed +
    // signing key exist.
    false
}

fn default_selected_language() -> String {
    "auto".to_string()
}

fn default_overlay_position() -> OverlayPosition {
    #[cfg(target_os = "linux")]
    return OverlayPosition::None;
    #[cfg(not(target_os = "linux"))]
    return OverlayPosition::Bottom;
}

fn default_debug_mode() -> bool {
    false
}

fn default_log_level() -> LogLevel {
    LogLevel::Debug
}

fn default_word_correction_threshold() -> f64 {
    0.18
}

fn default_paste_delay_ms() -> u64 {
    60
}

fn default_auto_submit() -> bool {
    false
}

fn default_history_limit() -> usize {
    5
}

fn default_recording_retention_period() -> RecordingRetentionPeriod {
    RecordingRetentionPeriod::PreserveLimit
}

fn default_audio_feedback_volume() -> f32 {
    1.0
}

fn default_sound_theme() -> SoundTheme {
    SoundTheme::Marimba
}

fn default_post_process_enabled() -> bool {
    false
}

fn default_app_language() -> String {
    tauri_plugin_os::locale()
        .map(|l| l.replace('_', "-"))
        .unwrap_or_else(|| "en".to_string())
}

fn default_show_tray_icon() -> bool {
    true
}

fn default_post_process_provider_id() -> String {
    "openai".to_string()
}

/// The post-processing provider that talks to Gemini. Its credential lives in
/// [`AppSettings::gemini_api_key`], not in `post_process_api_keys`.
pub const GOOGLE_PROVIDER_ID: &str = "google";

fn default_post_process_providers() -> Vec<PostProcessProvider> {
    let mut providers = vec![
        PostProcessProvider {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
        },
        PostProcessProvider {
            id: "zai".to_string(),
            label: "Z.AI".to_string(),
            base_url: "https://api.z.ai/api/paas/v4".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
        },
        PostProcessProvider {
            id: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
        },
        PostProcessProvider {
            id: GOOGLE_PROVIDER_ID.to_string(),
            label: "Google Gemini".to_string(),
            // Google's OpenAI-compatible layer, so this rides the existing
            // chat-completions path rather than needing a second client.
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
        },
        PostProcessProvider {
            id: "anthropic".to_string(),
            label: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
        },
        PostProcessProvider {
            id: "groq".to_string(),
            label: "Groq".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
        },
        PostProcessProvider {
            id: "cerebras".to_string(),
            label: "Cerebras".to_string(),
            base_url: "https://api.cerebras.ai/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
        },
    ];

    // Note: We always include Apple Intelligence on macOS ARM64 without checking availability
    // at startup. The availability check is deferred to when the user actually tries to use it
    // (in actions.rs). This prevents crashes on macOS 26.x beta where accessing
    // SystemLanguageModel.default during early app initialization causes SIGABRT.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        providers.push(PostProcessProvider {
            id: APPLE_INTELLIGENCE_PROVIDER_ID.to_string(),
            label: "Apple Intelligence".to_string(),
            base_url: "apple-intelligence://local".to_string(),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: true,
        });
    }

    // Custom provider always comes last
    providers.push(PostProcessProvider {
        id: "custom".to_string(),
        label: "Custom".to_string(),
        base_url: "http://localhost:11434/v1".to_string(),
        allow_base_url_edit: true,
        models_endpoint: Some("/models".to_string()),
        supports_structured_output: false,
    });

    providers
}

fn default_post_process_api_keys() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for provider in default_post_process_providers() {
        // Google's key lives in `gemini_api_key`; see `post_process_key_for`.
        if provider.id == GOOGLE_PROVIDER_ID {
            continue;
        }
        map.insert(provider.id, String::new());
    }
    map
}

fn default_model_for_provider(provider_id: &str) -> String {
    if provider_id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return APPLE_INTELLIGENCE_DEFAULT_MODEL_ID.to_string();
    }
    String::new()
}

fn default_post_process_models() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for provider in default_post_process_providers() {
        map.insert(
            provider.id.clone(),
            default_model_for_provider(&provider.id),
        );
    }
    map
}

fn default_post_process_prompts() -> Vec<LLMPrompt> {
    vec![LLMPrompt {
        id: "default_improve_transcriptions".to_string(),
        name: "Improve Transcriptions".to_string(),
        prompt: "Clean this transcript:\n1. Fix spelling, capitalization, and punctuation errors\n2. Convert number words to digits (twenty-five → 25, ten percent → 10%, five dollars → $5)\n3. Replace spoken punctuation with symbols (period → ., comma → ,, question mark → ?)\n4. Remove filler words (um, uh, like as filler)\n5. Keep the language in the original version (if it was french, keep it in french for example)\n\nPreserve exact meaning and word order. Do not paraphrase or reorder content.\n\nReturn only the cleaned transcript.\n\nTranscript:\n${output}".to_string(),
    }]
}

fn default_typing_tool() -> TypingTool {
    TypingTool::Auto
}

/// Move installs that had "finalize with Gemini" switched on onto the Gemini
/// meeting model, so the feature keeps working after the toggle was removed.
///
/// Only picks the model when the user has not already chosen one — an explicit
/// choice must never be overwritten by a migration. Returns whether anything
/// changed, so the caller knows to persist.
fn migrate_gemini_finalize_to_meeting_model(settings: &mut AppSettings) -> bool {
    if !settings.meeting_gemini_finalize {
        return false;
    }
    settings.meeting_gemini_finalize = false;
    if settings.meeting_selected_model.trim().is_empty() {
        settings.meeting_selected_model = crate::managers::model::GEMINI_MODEL_ID.to_string();
        debug!("Migrated 'finalize with Gemini' onto the Gemini meeting model");
    }
    true
}

/// Fold the old meeting-only vocabulary into the shared `custom_words` list.
///
/// Order is preserved and existing entries win, so a term present in both does
/// not end up duplicated. Returns whether anything changed.
fn migrate_meeting_vocabulary_into_custom_words(settings: &mut AppSettings) -> bool {
    if settings.meeting_custom_vocabulary.trim().is_empty() {
        return false;
    }
    let extra = crate::gemini_transcribe::parse_vocabulary(&settings.meeting_custom_vocabulary);
    settings.meeting_custom_vocabulary = String::new();

    let mut added = 0usize;
    for term in extra {
        if !settings
            .custom_words
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&term))
        {
            settings.custom_words.push(term);
            added += 1;
        }
    }
    if added > 0 {
        debug!("Merged {added} meeting vocabulary term(s) into custom words");
    }
    true
}

/// Fold a second copy of the Google key back into [`AppSettings::gemini_api_key`].
///
/// The AI-editing tab used to offer its own key field for the Google provider,
/// so installs can carry the same credential twice. Keep whichever copy is
/// populated (the dedicated field wins, since that is the one the Models page
/// still writes) and drop the map entry, so `post_process_key_for` has exactly
/// one place to look.
fn migrate_google_post_process_key_into_gemini_key(settings: &mut AppSettings) -> bool {
    let Some(stray) = settings.post_process_api_keys.remove(GOOGLE_PROVIDER_ID) else {
        return false;
    };
    let stray = stray.trim();
    if !stray.is_empty() && settings.gemini_api_key.trim().is_empty() {
        settings.gemini_api_key = stray.to_string();
        debug!("Moved the Google post-processing key into gemini_api_key");
    }
    true
}

fn ensure_post_process_defaults(settings: &mut AppSettings) -> bool {
    let mut changed = false;
    for provider in default_post_process_providers() {
        // Use match to do a single lookup - either sync existing or add new
        match settings
            .post_process_providers
            .iter_mut()
            .find(|p| p.id == provider.id)
        {
            Some(existing) => {
                // Sync supports_structured_output field for existing providers (migration)
                if existing.supports_structured_output != provider.supports_structured_output {
                    debug!(
                        "Updating supports_structured_output for provider '{}' from {} to {}",
                        provider.id,
                        existing.supports_structured_output,
                        provider.supports_structured_output
                    );
                    existing.supports_structured_output = provider.supports_structured_output;
                    changed = true;
                }
            }
            None => {
                // Provider doesn't exist, add it
                settings.post_process_providers.push(provider.clone());
                changed = true;
            }
        }

        // Google has no map entry by design (its key is `gemini_api_key`).
        // Inserting one here would be removed again by the Google-key
        // migration, reporting a change — and a store write — on every read.
        if provider.id != GOOGLE_PROVIDER_ID
            && !settings.post_process_api_keys.contains_key(&provider.id)
        {
            settings
                .post_process_api_keys
                .insert(provider.id.clone(), String::new());
            changed = true;
        }

        let default_model = default_model_for_provider(&provider.id);
        match settings.post_process_models.get_mut(&provider.id) {
            Some(existing) => {
                if existing.is_empty() && !default_model.is_empty() {
                    *existing = default_model.clone();
                    changed = true;
                }
            }
            None => {
                settings
                    .post_process_models
                    .insert(provider.id.clone(), default_model);
                changed = true;
            }
        }
    }

    changed
}

pub const SETTINGS_STORE_PATH: &str = "settings_store.json";

pub fn get_default_settings() -> AppSettings {
    #[cfg(target_os = "windows")]
    let default_shortcut = "ctrl+space";
    #[cfg(target_os = "macos")]
    let default_shortcut = "option+space";
    #[cfg(target_os = "linux")]
    let default_shortcut = "ctrl+space";
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let default_shortcut = "alt+space";

    let mut bindings = HashMap::new();
    bindings.insert(
        "transcribe".to_string(),
        ShortcutBinding {
            id: "transcribe".to_string(),
            name: "Transcribe".to_string(),
            description: "Converts your speech into text.".to_string(),
            default_binding: default_shortcut.to_string(),
            current_binding: default_shortcut.to_string(),
        },
    );
    #[cfg(target_os = "windows")]
    let default_post_process_shortcut = "ctrl+shift+space";
    #[cfg(target_os = "macos")]
    let default_post_process_shortcut = "option+shift+space";
    #[cfg(target_os = "linux")]
    let default_post_process_shortcut = "ctrl+shift+space";
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let default_post_process_shortcut = "alt+shift+space";

    bindings.insert(
        "transcribe_with_post_process".to_string(),
        ShortcutBinding {
            id: "transcribe_with_post_process".to_string(),
            name: "Transcribe with Post-Processing".to_string(),
            description: "Converts your speech into text and applies AI post-processing."
                .to_string(),
            default_binding: default_post_process_shortcut.to_string(),
            current_binding: default_post_process_shortcut.to_string(),
        },
    );
    bindings.insert(
        "cancel".to_string(),
        ShortcutBinding {
            id: "cancel".to_string(),
            name: "Cancel".to_string(),
            description: "Cancels the current recording.".to_string(),
            default_binding: "escape".to_string(),
            current_binding: "escape".to_string(),
        },
    );
    // Opt-in (no default binding): toggle a meeting recording on/off without
    // opening the window. Unbound by default so it never collides with the
    // dictation shortcut; the user assigns a key in Settings. The empty binding
    // is skipped by the shortcut registration loops.
    bindings.insert(
        "toggle_meeting".to_string(),
        ShortcutBinding {
            id: "toggle_meeting".to_string(),
            name: "Start/Stop Meeting".to_string(),
            description: "Starts or stops a meeting recording.".to_string(),
            default_binding: "".to_string(),
            current_binding: "".to_string(),
        },
    );

    AppSettings {
        bindings,
        push_to_talk: true,
        audio_feedback: false,
        audio_feedback_volume: default_audio_feedback_volume(),
        sound_theme: default_sound_theme(),
        start_hidden: default_start_hidden(),
        autostart_enabled: default_autostart_enabled(),
        update_checks_enabled: default_update_checks_enabled(),
        selected_model: "".to_string(),
        always_on_microphone: false,
        selected_microphone: None,
        clamshell_microphone: None,
        selected_output_device: None,
        translate_to_english: false,
        selected_language: "auto".to_string(),
        overlay_position: default_overlay_position(),
        debug_mode: false,
        log_level: default_log_level(),
        custom_words: Vec::new(),
        model_unload_timeout: ModelUnloadTimeout::default(),
        word_correction_threshold: default_word_correction_threshold(),
        history_limit: default_history_limit(),
        recording_retention_period: default_recording_retention_period(),
        paste_method: PasteMethod::default(),
        clipboard_handling: ClipboardHandling::default(),
        auto_submit: default_auto_submit(),
        auto_submit_key: AutoSubmitKey::default(),
        post_process_enabled: default_post_process_enabled(),
        post_process_provider_id: default_post_process_provider_id(),
        post_process_providers: default_post_process_providers(),
        post_process_api_keys: default_post_process_api_keys(),
        post_process_models: default_post_process_models(),
        post_process_prompts: default_post_process_prompts(),
        post_process_selected_prompt_id: None,
        openrouter_custom_model: String::new(),
        mute_while_recording: false,
        append_trailing_space: false,
        app_language: default_app_language(),
        experimental_enabled: false,
        lazy_stream_close: false,
        keyboard_implementation: KeyboardImplementation::default(),
        show_tray_icon: default_show_tray_icon(),
        paste_delay_ms: default_paste_delay_ms(),
        typing_tool: default_typing_tool(),
        external_script_path: None,
        custom_filler_words: None,
        whisper_accelerator: WhisperAcceleratorSetting::default(),
        ort_accelerator: OrtAcceleratorSetting::default(),
        extra_recording_buffer_ms: 0,
        meeting_auto_summarize: false,
        meeting_selected_model: String::new(),
        meeting_final_model: default_meeting_final_model(),
        meeting_language: default_meeting_language(),
        meeting_summary_templates: default_meeting_summary_templates(),
        meeting_auto_detect: false,
        meeting_auto_end: default_meeting_auto_end(),
        meeting_calendar_names: false,
        meeting_export_dir: String::new(),
        meeting_silence_timeout_secs: default_meeting_silence_timeout_secs(),
        meeting_auto_end_grace_secs: default_meeting_auto_end_grace_secs(),
        meeting_live_mode: default_meeting_live_mode(),
        meeting_live_translate_target: default_meeting_live_translate_target(),
        meeting_live_translate_model: default_meeting_live_translate_model(),
        meeting_live_transcribe_model: default_meeting_live_transcribe_model(),
        meeting_gemini_finalize: false,
        meeting_gemini_finalize_model: default_meeting_gemini_finalize_model(),
        meeting_gemini_diarize: true,
        meeting_gemini_smart: true,
        meeting_subtitles: true,
        meeting_custom_vocabulary: String::new(),
        gemini_api_key: String::new(),
    }
}

impl AppSettings {
    /// API key for a post-processing provider.
    ///
    /// Google reads [`Self::gemini_api_key`] and nothing else. That key already
    /// has a home on the Models page for transcription, and a second Google
    /// entry under `post_process_api_keys` would be the same credential stored
    /// twice — so whichever copy the user edited last would decide whether
    /// post-processing worked, with no way to tell from the UI which one won.
    /// [`migrate_google_post_process_key_into_gemini_key`] folds any existing
    /// second copy back in.
    pub fn post_process_key_for(&self, provider_id: &str) -> String {
        if provider_id == GOOGLE_PROVIDER_ID {
            return self.gemini_api_key.trim().to_string();
        }
        self.post_process_api_keys
            .get(provider_id)
            .map(|k| k.trim().to_string())
            .unwrap_or_default()
    }

    /// Which model transcribes meetings.
    ///
    /// [`Self::meeting_selected_model`] when the user picked one, otherwise the
    /// dictation model. Every meeting code path must go through this rather
    /// than reading `selected_model`, or the two selections silently diverge:
    /// the live pass would transcribe with one model while the loader warmed up
    /// another.
    pub fn meeting_model_id(&self) -> &str {
        let explicit = self.meeting_selected_model.trim();
        if explicit.is_empty() {
            &self.selected_model
        } else {
            explicit
        }
    }

    pub fn active_post_process_provider(&self) -> Option<&PostProcessProvider> {
        self.post_process_providers
            .iter()
            .find(|provider| provider.id == self.post_process_provider_id)
    }

    #[allow(dead_code)]
    pub fn post_process_provider(&self, provider_id: &str) -> Option<&PostProcessProvider> {
        self.post_process_providers
            .iter()
            .find(|provider| provider.id == provider_id)
    }

    pub fn post_process_provider_mut(
        &mut self,
        provider_id: &str,
    ) -> Option<&mut PostProcessProvider> {
        self.post_process_providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
    }
}

/// Parse a stored settings object without discarding more than it must.
///
/// The strict parse is tried first. When it fails, every top-level field of the
/// stored object is laid over the defaults one at a time and kept only if the
/// result still parses, so one malformed field (a renamed enum variant, a wrong
/// type written by an older build) resets that field alone instead of taking
/// the API keys, prompts and shortcuts down with it. Object-valued fields that
/// fail as a whole are retried entry by entry for the same reason.
///
/// Returns the settings plus the names of the fields that had to be reset; an
/// empty list means the strict parse succeeded.
pub(crate) fn parse_settings_lenient(raw: &serde_json::Value) -> (AppSettings, Vec<String>) {
    if let Ok(settings) = serde_json::from_value::<AppSettings>(raw.clone()) {
        return (settings, Vec::new());
    }

    let defaults = get_default_settings();
    let Some(raw_obj) = raw.as_object() else {
        return (defaults, vec!["<root>".to_string()]);
    };
    let Ok(mut merged) = serde_json::to_value(&defaults) else {
        return (defaults, vec!["<root>".to_string()]);
    };

    let parses =
        |candidate: &serde_json::Value| serde_json::from_value::<AppSettings>(candidate.clone()).is_ok();

    let mut reset = Vec::new();
    for (key, value) in raw_obj {
        let Some(default_value) = merged.get(key).cloned() else {
            // Unknown key (a field this build no longer has): ignored, exactly
            // as the strict parse would.
            continue;
        };

        let mut candidate = merged.clone();
        candidate[key] = value.clone();
        if parses(&candidate) {
            merged = candidate;
            continue;
        }

        // Whole field rejected. For maps (bindings, API keys, models) keep the
        // individual entries that are still valid.
        let mut kept_any = false;
        if let (Some(value_obj), Some(_)) = (value.as_object(), default_value.as_object()) {
            for (sub_key, sub_value) in value_obj {
                let mut candidate = merged.clone();
                candidate[key][sub_key] = sub_value.clone();
                if parses(&candidate) {
                    merged = candidate;
                    kept_any = true;
                }
            }
        }
        reset.push(if kept_any {
            format!("{key} (partially)")
        } else {
            key.clone()
        });
    }

    match serde_json::from_value::<AppSettings>(merged) {
        Ok(settings) => (settings, reset),
        // Every step above was checked to parse, so this is unreachable in
        // practice; defaults remain the only safe answer if it ever happens.
        Err(_) => (defaults, vec!["<root>".to_string()]),
    }
}

/// File name for a backup of unreadable settings taken at `stamp`.
pub(crate) fn settings_backup_file_name(stamp: &chrono::DateTime<chrono::Local>) -> String {
    format!(
        "{}.bak.{}",
        SETTINGS_STORE_PATH,
        stamp.format("%Y%m%d-%H%M%S")
    )
}

/// Copy the stored settings JSON aside before it is rewritten after a parse
/// failure, so nothing the user configured is lost for good even when the
/// lenient merge had to drop a field.
fn backup_raw_settings(app: &AppHandle, raw: &serde_json::Value) {
    let dir = match crate::portable::app_data_dir(app) {
        Ok(dir) => dir,
        Err(e) => {
            log::error!("Cannot back up unreadable settings: no app data dir ({e})");
            return;
        }
    };
    let path = dir.join(settings_backup_file_name(&chrono::Local::now()));
    let body = serde_json::to_string_pretty(raw).unwrap_or_else(|_| raw.to_string());
    match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, body)) {
        Ok(()) => warn!("Backed up the unreadable settings to {}", path.display()),
        Err(e) => log::error!("Failed to back up settings to {}: {e}", path.display()),
    }
}

/// Apply every load-time migration. Returns whether anything changed.
fn apply_migrations(settings: &mut AppSettings) -> bool {
    let mut migrated = ensure_post_process_defaults(settings);
    migrated |= migrate_gemini_finalize_to_meeting_model(settings);
    migrated |= migrate_meeting_vocabulary_into_custom_words(settings);
    migrated |= migrate_google_post_process_key_into_gemini_key(settings);
    migrated
}

// --- Write serialization -----------------------------------------------------
//
// Every settings change is a read-modify-write of one JSON blob. Two commands
// racing each other (the frontend fires several from one page) would otherwise
// each read the old blob, and the second write would silently undo the first.
// The lock is re-entrant per thread so a helper that writes from inside an
// `update_settings` closure cannot deadlock itself.

struct WriteLockState {
    owner: Option<std::thread::ThreadId>,
    depth: usize,
}

static WRITE_LOCK: std::sync::Mutex<WriteLockState> = std::sync::Mutex::new(WriteLockState {
    owner: None,
    depth: 0,
});
static WRITE_LOCK_CV: std::sync::Condvar = std::sync::Condvar::new();

struct WriteLockGuard;

fn lock_settings_writes() -> WriteLockGuard {
    let me = std::thread::current().id();
    let mut state = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        match state.owner {
            None => {
                state.owner = Some(me);
                state.depth = 1;
                break;
            }
            Some(owner) if owner == me => {
                state.depth += 1;
                break;
            }
            Some(_) => {
                state = WRITE_LOCK_CV
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
    }
    WriteLockGuard
}

impl Drop for WriteLockGuard {
    fn drop(&mut self) {
        let mut state = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            state.owner = None;
            WRITE_LOCK_CV.notify_one();
        }
    }
}

fn settings_store(app: &AppHandle) -> std::sync::Arc<tauri_plugin_store::Store<tauri::Wry>> {
    app.store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store")
}

fn store_settings(store: &tauri_plugin_store::Store<tauri::Wry>, settings: &AppSettings) {
    match serde_json::to_value(settings) {
        Ok(value) => store.set("settings", value),
        Err(e) => log::error!("Failed to serialize settings: {e}"),
    }
}

/// Read the settings out of the store, repairing (and persisting) them when the
/// stored copy is missing, unreadable or needs a migration. Writes back only
/// when something actually changed.
fn read_settings(app: &AppHandle, merge_default_bindings: bool) -> AppSettings {
    let store = settings_store(app);
    let (settings, dirty) = compute_settings(app, &store, merge_default_bindings, false);
    if !dirty {
        return settings;
    }

    // Repair needed. Redo it under the write lock against a fresh read, so a
    // writer that landed since the read above is not overwritten with stale
    // data.
    let _guard = lock_settings_writes();
    let (settings, dirty) = compute_settings(app, &store, merge_default_bindings, true);
    if dirty {
        store_settings(&store, &settings);
    }
    settings
}

/// Parse + repair the stored settings. Returns them with whether they differ
/// from what is stored. Backs up an unreadable blob only when `backup` is set,
/// so the unlocked first pass in [`read_settings`] never writes anything.
fn compute_settings(
    app: &AppHandle,
    store: &tauri_plugin_store::Store<tauri::Wry>,
    merge_default_bindings: bool,
    backup: bool,
) -> (AppSettings, bool) {
    let (mut settings, mut dirty) = match store.get("settings") {
        Some(raw) => {
            let (settings, reset) = parse_settings_lenient(&raw);
            if reset.is_empty() {
                (settings, false)
            } else {
                if backup {
                    log::error!(
                        "Stored settings could not be read in full; reset to defaults: {}",
                        reset.join(", ")
                    );
                    backup_raw_settings(app, &raw);
                }
                (settings, true)
            }
        }
        None => (get_default_settings(), true),
    };

    if merge_default_bindings {
        for (key, value) in get_default_settings().bindings {
            if !settings.bindings.contains_key(&key) {
                debug!("Adding missing binding: {}", key);
                settings.bindings.insert(key, value);
                dirty = true;
            }
        }
    }

    dirty |= apply_migrations(&mut settings);

    (settings, dirty)
}

pub fn load_or_create_app_settings(app: &AppHandle) -> AppSettings {
    let _guard = lock_settings_writes();
    read_settings(app, true)
}

pub fn get_settings(app: &AppHandle) -> AppSettings {
    read_settings(app, false)
}

/// Overwrite the stored settings. Prefer [`update_settings`], which reads the
/// current value under the same lock and so cannot lose a concurrent change.
#[allow(dead_code)] // kept for callers on other branches; new code uses update_settings
pub fn write_settings(app: &AppHandle, settings: AppSettings) {
    let _guard = lock_settings_writes();
    store_settings(&settings_store(app), &settings);
}

/// Read-modify-write the settings atomically with respect to every other
/// writer in the process.
pub fn update_settings<R>(app: &AppHandle, f: impl FnOnce(&mut AppSettings) -> R) -> R {
    let _guard = lock_settings_writes();
    let mut settings = read_settings(app, false);
    let result = f(&mut settings);
    store_settings(&settings_store(app), &settings);
    result
}

/// [`update_settings`] for a change that can be rejected: nothing is written
/// when the closure returns `Err`.
pub fn try_update_settings<R, E>(
    app: &AppHandle,
    f: impl FnOnce(&mut AppSettings) -> Result<R, E>,
) -> Result<R, E> {
    let _guard = lock_settings_writes();
    let mut settings = read_settings(app, false);
    let result = f(&mut settings)?;
    store_settings(&settings_store(app), &settings);
    Ok(result)
}

pub fn get_bindings(app: &AppHandle) -> HashMap<String, ShortcutBinding> {
    let settings = get_settings(app);

    settings.bindings
}

pub fn get_stored_binding(app: &AppHandle, id: &str) -> ShortcutBinding {
    let bindings = get_bindings(app);

    let binding = bindings.get(id).unwrap().clone();

    binding
}

pub fn get_history_limit(app: &AppHandle) -> usize {
    let settings = get_settings(app);
    settings.history_limit
}

pub fn get_recording_retention_period(app: &AppHandle) -> RecordingRetentionPeriod {
    let settings = get_settings(app);
    settings.recording_retention_period
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_post_process_key_moves_into_the_gemini_field() {
        let mut settings = get_default_settings();
        settings
            .post_process_api_keys
            .insert("google".to_string(), "  stray-key  ".to_string());

        assert!(migrate_google_post_process_key_into_gemini_key(
            &mut settings
        ));
        assert_eq!(settings.gemini_api_key, "stray-key");
        assert!(!settings.post_process_api_keys.contains_key("google"));
        // Idempotent: nothing left to move on the next load.
        assert!(!migrate_google_post_process_key_into_gemini_key(
            &mut settings
        ));
    }

    #[test]
    fn an_existing_gemini_key_wins_over_the_stray_copy() {
        let mut settings = get_default_settings();
        settings.gemini_api_key = "models-page-key".to_string();
        settings
            .post_process_api_keys
            .insert("google".to_string(), "stale-copy".to_string());

        assert!(migrate_google_post_process_key_into_gemini_key(
            &mut settings
        ));
        assert_eq!(settings.gemini_api_key, "models-page-key");
        assert!(!settings.post_process_api_keys.contains_key("google"));
    }

    #[test]
    fn post_process_key_for_google_reads_only_the_gemini_field() {
        let mut settings = get_default_settings();
        settings.gemini_api_key = "gemini-key".to_string();
        settings
            .post_process_api_keys
            .insert("openrouter".to_string(), "or-key".to_string());

        assert_eq!(settings.post_process_key_for("google"), "gemini-key");
        assert_eq!(settings.post_process_key_for("openrouter"), "or-key");
        assert_eq!(settings.post_process_key_for("groq"), "");
    }

    #[test]
    fn the_old_gemini_finalize_toggle_becomes_the_gemini_meeting_model() {
        let mut settings = get_default_settings();
        settings.meeting_gemini_finalize = true;

        assert!(migrate_gemini_finalize_to_meeting_model(&mut settings));
        assert_eq!(
            settings.meeting_selected_model,
            crate::managers::model::GEMINI_MODEL_ID
        );
        // The flag is cleared, so the migration is not re-run on every load.
        assert!(!settings.meeting_gemini_finalize);
        assert!(!migrate_gemini_finalize_to_meeting_model(&mut settings));
    }

    #[test]
    fn the_migration_never_overwrites_a_chosen_meeting_model() {
        let mut settings = get_default_settings();
        settings.meeting_gemini_finalize = true;
        settings.meeting_selected_model = "turbo".to_string();

        assert!(migrate_gemini_finalize_to_meeting_model(&mut settings));
        assert_eq!(settings.meeting_selected_model, "turbo");
        assert!(!settings.meeting_gemini_finalize);
    }

    #[test]
    fn installs_without_the_old_toggle_are_left_alone() {
        let mut settings = get_default_settings();
        assert!(!migrate_gemini_finalize_to_meeting_model(&mut settings));
        assert_eq!(settings.meeting_selected_model, "");
    }

    #[test]
    fn meetings_follow_the_dictation_model_until_told_otherwise() {
        let mut settings = get_default_settings();
        settings.selected_model = "parakeet-tdt-0.6b-v3".to_string();

        // Unset is the default, and must keep tracking `selected_model` rather
        // than pinning whatever it happened to be at first run.
        assert_eq!(settings.meeting_model_id(), "parakeet-tdt-0.6b-v3");
        settings.selected_model = "turbo".to_string();
        assert_eq!(settings.meeting_model_id(), "turbo");
    }

    #[test]
    fn an_explicit_meeting_model_wins_and_whitespace_is_not_a_choice() {
        let mut settings = get_default_settings();
        settings.selected_model = "turbo".to_string();
        settings.meeting_selected_model = "gemini-transcribe".to_string();
        assert_eq!(settings.meeting_model_id(), "gemini-transcribe");

        // A blank field means "follow dictation", not "transcribe with nothing".
        settings.meeting_selected_model = "   ".to_string();
        assert_eq!(settings.meeting_model_id(), "turbo");
    }

    #[test]
    fn google_provider_falls_back_to_the_meeting_gemini_key() {
        let mut settings = get_default_settings();
        settings.gemini_api_key = "meeting-key".to_string();

        // Pasting the same Gemini key twice under two labels is the kind of
        // thing users do not do, and then post-processing is silently dead.
        assert_eq!(settings.post_process_key_for("google"), "meeting-key");
        // The fallback is Google-only; nothing else may borrow that key.
        assert_eq!(settings.post_process_key_for("openai"), "");
    }

    #[test]
    fn a_stray_google_map_entry_cannot_shadow_the_gemini_key() {
        let mut settings = get_default_settings();
        settings.gemini_api_key = "meeting-key".to_string();
        // An entry left behind by an older build must not decide which key is
        // used — that is exactly the two-copies confusion this removed.
        settings
            .post_process_api_keys
            .insert("google".to_string(), "stale-copy".to_string());
        assert_eq!(settings.post_process_key_for("google"), "meeting-key");
    }

    #[test]
    fn existing_installs_gain_the_google_provider_on_load() {
        // The provider list is PERSISTED, so adding one to the defaults does
        // nothing for existing users without this reconciliation.
        let mut settings = get_default_settings();
        settings.post_process_providers.retain(|p| p.id != "google");
        assert!(ensure_post_process_defaults(&mut settings));
        assert!(settings
            .post_process_providers
            .iter()
            .any(|p| p.id == "google"));
    }

    #[test]
    fn migrations_are_a_no_op_on_settings_that_need_none() {
        // Regression: the Google provider used to get a map entry inserted by
        // `ensure_post_process_defaults` and removed again by the key
        // migration, so every single read reported a change and rewrote the
        // store.
        let mut settings = get_default_settings();
        assert!(!settings.post_process_api_keys.contains_key("google"));
        assert!(!apply_migrations(&mut settings));
        assert!(!apply_migrations(&mut settings));
    }

    #[test]
    fn a_valid_blob_parses_strictly_with_nothing_reset() {
        let raw = serde_json::to_value(get_default_settings()).unwrap();
        let (_, reset) = parse_settings_lenient(&raw);
        assert!(reset.is_empty());
    }

    #[test]
    fn one_bad_field_resets_only_that_field() {
        let mut stored = get_default_settings();
        stored.gemini_api_key = "keep-me".to_string();
        stored
            .post_process_api_keys
            .insert("openrouter".to_string(), "or-key".to_string());
        stored.custom_words = vec!["Fısıltı".to_string()];
        stored.selected_model = "turbo".to_string();
        let mut raw = serde_json::to_value(&stored).unwrap();
        raw["overlay_position"] = serde_json::json!("sideways");
        raw["paste_delay_ms"] = serde_json::json!("not a number");

        let (settings, reset) = parse_settings_lenient(&raw);

        assert_eq!(settings.gemini_api_key, "keep-me");
        assert_eq!(settings.post_process_key_for("openrouter"), "or-key");
        assert_eq!(settings.custom_words, vec!["Fısıltı".to_string()]);
        assert_eq!(settings.selected_model, "turbo");
        assert_eq!(settings.overlay_position, default_overlay_position());
        assert_eq!(settings.paste_delay_ms, default_paste_delay_ms());
        assert_eq!(reset.len(), 2, "{reset:?}");
        assert!(reset.contains(&"overlay_position".to_string()));
        assert!(reset.contains(&"paste_delay_ms".to_string()));
    }

    #[test]
    fn a_bad_map_entry_keeps_the_good_entries() {
        let mut stored = get_default_settings();
        stored.bindings.get_mut("transcribe").unwrap().current_binding = "ctrl+k".to_string();
        let mut raw = serde_json::to_value(&stored).unwrap();
        raw["bindings"]["cancel"] = serde_json::json!({ "id": 5 });

        let (settings, reset) = parse_settings_lenient(&raw);

        assert_eq!(settings.bindings["transcribe"].current_binding, "ctrl+k");
        // The broken entry falls back to its default.
        assert_eq!(settings.bindings["cancel"].current_binding, "escape");
        assert_eq!(reset, vec!["bindings (partially)".to_string()]);
    }

    #[test]
    fn unknown_fields_are_ignored_rather_than_reported() {
        let mut raw = serde_json::to_value(get_default_settings()).unwrap();
        raw["overlay_position"] = serde_json::json!(42);
        raw["field_from_the_future"] = serde_json::json!(true);
        let (_, reset) = parse_settings_lenient(&raw);
        assert_eq!(reset, vec!["overlay_position".to_string()]);
    }

    #[test]
    fn a_non_object_blob_falls_back_to_defaults() {
        let (settings, reset) = parse_settings_lenient(&serde_json::json!("garbage"));
        assert_eq!(reset, vec!["<root>".to_string()]);
        assert_eq!(settings.selected_language, "auto");
    }

    #[test]
    fn backup_names_carry_a_sortable_timestamp() {
        use chrono::TimeZone;
        let stamp = chrono::Local
            .with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
            .unwrap();
        assert_eq!(
            settings_backup_file_name(&stamp),
            "settings_store.json.bak.20260304-050607"
        );
    }

    #[test]
    fn default_settings_disable_auto_submit() {
        let settings = get_default_settings();
        assert!(!settings.auto_submit);
        assert_eq!(settings.auto_submit_key, AutoSubmitKey::Enter);
    }
}
