// Browser preview of the main window with a fake Tauri backend, for design
// work without building the app: `bun run dev:preview`, then open
// http://localhost:1420/?lang=tr&state=idle|recording
// Never bundled into the app: vite.config.ts only injects it when
// VITE_TAURI_MOCK=1, ahead of main.tsx.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import defaults from "./default-settings.json";

const params = new URLSearchParams(location.search);
const lang = params.get("lang") ?? "tr";
const recording = params.get("state") === "recording";
const firstRun = params.get("state") === "empty";

const en = lang === "en";
const now = Date.now();
const day = 86_400_000;

const settings = {
  ...defaults,
  app_language: lang,
  selected_model: "large-v3-turbo",
};

const model = (
  id: string,
  name: string,
  description: string,
  engine_type: string,
  size_mb: number,
  is_downloaded: boolean,
  extra: Record<string, unknown> = {},
) => ({
  id,
  name,
  description,
  filename: id,
  url: null,
  sha256: null,
  size_mb,
  is_downloaded,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type,
  accuracy_score: 0.85,
  speed_score: 0.7,
  supports_translation: true,
  is_recommended: id === "large-v3-turbo",
  supported_languages: ["en", "tr", "de", "fr", "es"],
  supports_language_selection: true,
  is_custom: false,
  ...extra,
});

const models = [
  model(
    "large-v3-turbo",
    "Whisper Large v3 Turbo",
    "Fast and accurate, 99 languages.",
    "Whisper",
    1600,
    true,
  ),
  model(
    "parakeet-v3",
    "Parakeet V3",
    "Very fast, 25 European languages.",
    "Parakeet",
    480,
    false,
  ),
  model("small", "Whisper Small", "Light and quick.", "Whisper", 487, false),
  model(
    "openrouter-asr-gpt-4o-mini-transcribe",
    "GPT-4o mini Transcribe",
    "Cloud, via OpenRouter.",
    "OpenRouterAsr",
    0,
    true,
  ),
];

const meetings = firstRun
  ? []
  : [
      {
        id: 3,
        started_at: now - 2 * 3600_000,
        ended_at: now - 3600_000 - 18 * 60_000,
        duration_ms: 42 * 60_000,
        title: en ? "Q4 product roadmap" : "Q4 ürün yol haritası",
        has_summary: true,
        transcript_preview: en
          ? "We need pricing settled by the end of October — three tiers on the enterprise side…"
          : "Fiyatlandırmayı ekim sonuna kadar netleştirmemiz lazım, kurumsal tarafta üç seviye…",
        status: "completed",
      },
      {
        id: 2,
        started_at: now - day - 5 * 3600_000,
        ended_at: now - day - 4 * 3600_000,
        duration_ms: 38 * 60_000,
        title: en
          ? "Customer call — pilot scope"
          : "Müşteri görüşmesi — pilot kapsamı",
        has_summary: true,
        transcript_preview: en
          ? "Agreed on the integration timeline and the first two weeks of the pilot."
          : "Entegrasyon takvimi ve pilotun ilk iki haftası üzerinde anlaştık.",
        status: "completed",
      },
      {
        id: 1,
        started_at: now - 3 * day,
        ended_at: now - 3 * day + 25 * 60_000,
        duration_ms: 25 * 60_000,
        title: en ? "1:1 — Maya" : "1:1 — Mehmet",
        has_summary: false,
        transcript_preview: en
          ? "Career goals, Q4 priorities and the holiday plan."
          : "Kariyer hedefleri, Q4 öncelikleri ve izin planı.",
        status: "completed",
      },
    ];

const history = firstRun
  ? []
  : [
      {
        id: 2,
        file_name: "a.wav",
        timestamp: Math.floor((now - 20 * 60_000) / 1000),
        saved: false,
        title: "",
        transcription_text: en
          ? "I'll share the meeting notes with the team by tomorrow morning — shout if anything's missing."
          : "Toplantı notlarını yarın sabaha kadar ekiple paylaşıyorum, eksik bir şey varsa haber verin.",
        post_processed_text: null,
        post_process_prompt: null,
        post_process_requested: false,
      },
      {
        id: 1,
        file_name: "b.wav",
        timestamp: Math.floor((now - 3 * 3600_000) / 1000),
        saved: false,
        title: "",
        transcription_text: en
          ? "Let's review the pricing proposal before Monday."
          : "Fiyat teklifini pazartesiye kadar gözden geçirelim.",
        post_processed_text: null,
        post_process_prompt: null,
        post_process_requested: false,
      },
    ];

const segments = recording
  ? [
      {
        text: en
          ? "We're thinking three enterprise tiers — just don't make the entry one too cheap."
          : "Kurumsal tarafta üç seviye düşünüyoruz, giriş seviyesi çok ucuz kalmasın.",
        timestamp_ms: 12_000,
        source: "others",
      },
      {
        text: en
          ? "Summaries should stay in Pro — that's where the value is."
          : "Özetler bence Pro'da kalmalı, asıl değer orada.",
        timestamp_ms: 30_000,
        source: "mic",
      },
      {
        text: en
          ? "Makes sense. Then mobile moves out a quarter."
          : "Mantıklı. Mobil tarafı o zaman bir çeyrek kaydırıyoruz.",
        timestamp_ms: 48_000,
        source: "others",
      },
    ]
  : [];

const session = recording
  ? { state: "running", meeting_id: 4, started_at_ms: now - 24 * 60_000 }
  : { state: "idle", meeting_id: null, started_at_ms: null };

const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
  get_app_settings: () => settings,
  get_default_settings: () => defaults,
  get_available_models: () => models,
  get_current_model: () => settings.selected_model,
  has_any_models_available: () => true,
  has_any_models_or_downloads: () => true,
  get_transcription_model_status: () => settings.selected_model,
  get_model_load_status: () => ({
    is_loaded: true,
    current_model: settings.selected_model,
  }),
  get_available_microphones: () => [
    { index: "0", name: "Default", is_default: true },
    { index: "1", name: "MacBook Pro Mikrofonu", is_default: false },
  ],
  get_available_output_devices: () => [
    { index: "0", name: "Default", is_default: true },
  ],
  get_selected_microphone: () => "Default",
  get_selected_output_device: () => "Default",
  check_custom_sounds: () => ({ start: false, stop: false }),
  is_recording: () => false,
  is_laptop: () => true,
  get_clamshell_microphone: () => "Default",
  list_meetings: () => meetings,
  list_interrupted_meetings: () => [],
  get_meeting_session: () => session,
  get_meeting_status: () => session.state,
  get_meeting_started_at: () => session.started_at_ms,
  get_meeting_transcript: () => segments.map((s) => s.text).join("\n"),
  get_history_entries: () => ({ entries: history, has_more: false }),
  get_calendar_access_status: () => "authorized",
  get_transcription_location: () => ({ cloud_providers: [] }),
  get_supported_import_extensions: () => ["mp3", "m4a", "wav"],
  get_meeting_import_progress: () => null,
  get_windows_microphone_permission_status: () => ({ supported: false }),
  get_app_dir_path: () => "~/Library/Application Support/Fısıltı",
  get_log_dir_path: () => "~/Library/Logs/Fısıltı",
  "plugin:macos-permissions|check_accessibility_permission": () => true,
  "plugin:macos-permissions|check_microphone_permission": () => true,
  "plugin:app|version": () => "0.1.0",
  "plugin:autostart|is_enabled": () => false,
};

function installTauriMock() {
  // plugin-os reads these synchronously.
  (window as unknown as Record<string, unknown>).__TAURI_OS_PLUGIN_INTERNALS__ =
    {
      platform: "macos",
      os_type: "macos",
      family: "unix",
      arch: "aarch64",
      version: "15.0",
      eol: "\n",
      exe_extension: "",
    };
  mockWindows("main");
  mockIPC(
    (cmd, payload) => {
      const handler = handlers[cmd];
      if (handler) return handler((payload ?? {}) as Record<string, unknown>);
      if (!cmd.startsWith("plugin:event") && !cmd.startsWith("change_")) {
        console.debug("[tauri-mock] unhandled", cmd);
      }
      return null;
    },
    { shouldMockEvents: true },
  );
}

installTauriMock();

// A live meeting in the preview gets a real-looking title, as the calendar
// lookup would give it a few seconds in.
if (recording) {
  void import("@tauri-apps/api/event").then(({ emit }) =>
    setTimeout(() => {
      void emit("meeting-title-update", {
        id: 4,
        title: en ? "Q4 product roadmap" : "Q4 ürün yol haritası",
      });
    }, 600),
  );
}
