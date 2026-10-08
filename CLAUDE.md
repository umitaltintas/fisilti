# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Development Commands

**Prerequisites:** [Rust](https://rustup.rs/) (latest stable), [Bun](https://bun.sh/)

```bash
# Install dependencies
bun install

# Run in development mode
bun run tauri dev
# If cmake error on macOS:
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev

# Build for production (ad-hoc signed; CI uses this)
bun run tauri build

# Build for LOCAL install (signs with the Developer ID identity in
# src-tauri/tauri.macsign.conf.json so macOS TCC permissions survive
# rebuilds — always prefer this when the build will be installed to
# /Applications)
bun run build:mac

# Same, plus Apple notarization (API key from ~/.fisilti-signing/notary.env)
bun run build:mac:release

# Linting and formatting (run before committing)
bun run lint              # ESLint for frontend
bun run lint:fix          # ESLint with auto-fix
bun run format            # Prettier + cargo fmt
bun run format:check      # Check formatting without changes
```

**Model Setup (Required for Development):**

```bash
mkdir -p src-tauri/resources/models
curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
```

## Architecture Overview

Fisilti is a cross-platform desktop speech-to-text app built with Tauri 2.x (Rust backend + React/TypeScript frontend).

### Backend Structure (src-tauri/src/)

- `lib.rs` - Main entry point, Tauri setup, manager initialization
- `managers/` - Core business logic:
  - `audio.rs` - Audio recording and device management
  - `model.rs` - Model downloading and management
  - `transcription.rs` - Speech-to-text processing pipeline
  - `history.rs` - Transcription history storage
- `audio_toolkit/` - Low-level audio processing:
  - `audio/` - Device enumeration, recording, resampling
  - `vad/` - Voice Activity Detection (Silero VAD)
- `commands/` - Tauri command handlers for frontend communication
- `shortcut.rs` - Global keyboard shortcut handling
- `settings.rs` - Application settings management

### Frontend Structure (src/)

- `App.tsx` - Main component with onboarding flow
- `components/settings/` - Settings UI (35+ files)
- `components/model-selector/` - Model management interface
- `components/onboarding/` - First-run experience
- `hooks/useSettings.ts`, `useModels.ts` - State management hooks
- `stores/settingsStore.ts` - Zustand store for settings
- `bindings.ts` - Auto-generated Tauri type bindings (via tauri-specta)
- `overlay/` - Recording overlay window code

### Key Patterns

**Manager Pattern:** Core functionality organized into managers (Audio, Model, Transcription) initialized at startup and managed via Tauri state.

**Command-Event Architecture:** Frontend → Backend via Tauri commands; Backend → Frontend via events.

**Pipeline Processing:** Audio → VAD → Whisper/Parakeet → Text output → Clipboard/Paste

**State Flow:** Zustand → Tauri Command → Rust State → Persistence (tauri-plugin-store)

**Settings writes:** go through `settings::update_settings(&app, |s| ..)` (or
`try_update_settings`), a re-entrant process-wide lock around read-modify-write.
Never `get_settings` + `write_settings`; two commands racing would drop one
change. Reading is lenient: one unparseable field resets only that field and
the raw blob is backed up to `settings_store.json.bak.<timestamp>`. Enum
variants with digits need explicit `#[serde(rename)]` (serde says `min2`,
specta would say `min_2`).

**Errors reach the user.** Dictation failures emit `dictation-error`
(`{ stage: transcription | paste | model_load | no_speech | recording,
message }`, `utils::emit_dictation_error`); the frontend toasts them. On the
frontend, specta commands return `{ status: "error" }` instead of throwing —
always check `result.status`; `settingsStore` rolls back only the failed key.

**Bindings:** `src/bindings.ts` is generated. Regenerate without launching the
app: `cd src-tauri && cargo test --lib export_typescript_bindings -- --ignored`.

## Internationalization (i18n)

All user-facing strings must use i18next translations. ESLint enforces this (no hardcoded strings in JSX).

**Adding new text:**

1. Add key to `src/i18n/locales/en/translation.json`
2. Use in component: `const { t } = useTranslation(); t('key.path')`

**File structure:**

```
src/i18n/
├── index.ts           # i18n setup
├── languages.ts       # Language metadata
└── locales/
    ├── en/translation.json  # English (source)
    ├── es/translation.json  # Spanish
    ├── fr/translation.json  # French
    └── vi/translation.json  # Vietnamese
```

## Code Style

**Rust:**

- Run `cargo fmt` and `cargo clippy` before committing
- Handle errors explicitly (avoid unwrap in production)
- Use descriptive names, add doc comments for public APIs

**TypeScript/React:**

- Strict TypeScript, avoid `any` types
- Functional components with hooks
- Tailwind CSS for styling
- Path aliases: `@/` → `./src/`

## Commit Guidelines

Use conventional commits:

- `feat:` new features
- `fix:` bug fixes
- `docs:` documentation
- `refactor:` code refactoring
- `chore:` maintenance

## CLI Parameters

Fisilti supports command-line parameters on all platforms for integration with scripts, window managers, and autostart configurations.

**Implementation files:**

- `src-tauri/src/cli.rs` - CLI argument definitions (clap derive)
- `src-tauri/src/main.rs` - Argument parsing before Tauri launch
- `src-tauri/src/lib.rs` - Applying CLI overrides (setup closure + single-instance callback)
- `src-tauri/src/signal_handle.rs` - `send_transcription_input()` reusable function

**Available flags:**

| Flag                     | Description                                                                        |
| ------------------------ | ---------------------------------------------------------------------------------- |
| `--toggle-transcription` | Toggle recording on/off on a running instance (via `tauri_plugin_single_instance`) |
| `--toggle-post-process`  | Toggle recording with post-processing on/off on a running instance                 |
| `--cancel`               | Cancel the current operation on a running instance                                 |
| `--start-hidden`         | Launch without showing the main window (tray icon still visible)                   |
| `--no-tray`              | Launch without the system tray icon (closing window quits the app)                 |
| `--debug`                | Enable debug mode with verbose (Trace) logging                                     |

**Key design decisions:**

- CLI flags are runtime-only overrides — they do NOT modify persisted settings
- Remote control flags (`--toggle-transcription`, `--toggle-post-process`, `--cancel`) work by launching a second instance that sends its args to the running instance via `tauri_plugin_single_instance`, then exits
- `send_transcription_input()` in `signal_handle.rs` is shared between signal handlers and CLI to avoid code duplication
- `CliArgs` is stored in Tauri managed state (`.manage()`) so it's accessible in `on_window_event` and other handlers

## Meeting Auto-Detection (macOS)

Opt-in feature: detect when a meeting app starts using the microphone, prompt
to start a transcription session, and offer to end (then auto-end) the session
on prolonged silence or when the meeting app releases the mic.

**Session lifecycle:** each meeting is one `meeting/session.rs` `Session`
owned by the manager's slot; the slot is `Idle → Running → Finalizing → Idle`.
`Finalizing` (stop in progress) refuses start/import/recover and keeps
`is_active()` true, so dictation never swaps the engine mid-finalize. Delete,
discard, recover and retranscribe refuse the live/finalizing meeting id. The
manager is split into `capture.rs`, `finalize.rs`, `live.rs`, `exclusive.rs`,
`buffers.rs` (capture buffers live in `{app_data}/meeting_buffers`), `dsp.rs`,
`text.rs`, `summarize.rs`. Quitting mid-meeting flushes and leaves the row
recoverable (`MeetingManager::shutdown`, bounded ~6 s). The frontend mirrors
the session in `src/stores/meetingStore.ts` (initialised once in `main.tsx`,
driven by `meeting-session-changed`), so live notes autosave to the
in-progress row and survive navigation.

**Implementation files:**

- `src-tauri/src/meeting_detector.rs` - poll thread (3s cadence) + pure
  `DetectionSm` state machine + the auto-end grace-timer controller
- `src-tauri/src/meeting_prompt.rs` - the small always-on-top clickable prompt
  window (label `meeting_prompt`); React page in `src/meeting-prompt/`
- `src-tauri/src/meeting/manager.rs` - prolonged-silence tracking in the
  capture loop (`silence_anchor`, reset by speech frames from either VAD)
- Settings UI: "Automatic meeting detection" section in
  `src/components/settings/meeting/MeetingSettings.tsx`; helpers in
  `src/lib/meeting.ts`

**Settings (`AppSettings`):** `meeting_auto_detect` (default false),
`meeting_auto_end` (default true), `meeting_silence_timeout_secs` (180),
`meeting_auto_end_grace_secs` (60).

**Key design decisions:**

- Detection uses **CoreAudio process objects** (macOS 14+, via `cidre`):
  `kAudioHardwarePropertyProcessObjectList` → per-process bundle id +
  `IsRunningInput`. A process from the allowlist in `MEETING_APPS`
  (dedicated apps + browsers, matched exact-or-dotted-prefix so helper
  subprocesses count) that is actively pulling mic input = "in a meeting".
  Our own PID is excluded (the capture tap makes Fisilti itself report input).
- Start prompt is debounced (2 polls ≈ 6s); dismissing snoozes until the
  signal clears; stopping a session while the app still holds the mic also
  snoozes (no instant re-prompt for the same meeting).
- The end prompt is armed once (generation counter guards the grace thread
  against stale timers); unanswered prompts auto-stop via the shared
  `stop_meeting_session` path so finalize/summary run normally.
- The prompt window is a plain focusable `WebviewWindowBuilder` window (NOT
  `tauri_nspanel` — the overlay is deliberately non-clickable, this one must
  accept clicks). A `meeting-prompt-ready` handshake re-emits the payload so
  the first show never races the page mount.
- Commands: `accept_meeting_prompt`, `dismiss_meeting_prompt`,
  `respond_meeting_auto_end`, `get_meeting_detection_status`. Events:
  `meeting-prompt-update`, `meeting-detection-changed`.
- Tray: the "Meetings" item opens the main window on the Meeting section via
  a `navigate-section` event (listener in `App.tsx`).

## Meeting Naming (macOS)

New sessions are titled after the real meeting instead of the datetime
default, in priority order: calendar event in progress (EventKit, opt-in
`meeting_calendar_names` setting → Calendars TCC prompt) → the detected
meeting app's window/tab title (AX API; reuses the accessibility permission,
cleaned of product suffixes and rejected when generic, e.g. "Zoom Meeting" or
a bare Meet room code) → the existing LLM auto-title on stop → datetime.

- `src-tauri/src/meeting_naming.rs` - calendar + window-title resolution and
  the pure `clean_window_title` heuristics (unit-tested)
- `MeetingManager::resolve_session_title` runs on a background thread at
  start (2 attempts, 12s apart); the resolved title names the in-progress row
  and suppresses the LLM auto-title on stop
- The `meeting-title-update` event carries `{ id, title }` so the UI renames
  only the matching meeting
- Commands: `get_calendar_access_status`, `request_calendar_access`,
  `change_meeting_calendar_names_setting`. The toggle lives in the meeting
  "Settings" tab (`MeetingPreferences.tsx`) and requests calendar access
  before persisting the setting

## Recording Import & Markdown Export (macOS)

**Import** turns a recording made elsewhere (phone voice memo, conference
recording) into a normal completed meeting.

- `src-tauri/src/meeting/import.rs` - symphonia decode → 16 kHz mono, the
  recording date (file tag → creation time → mtime − duration), and
  `title_from_path` (recorder default names like "New Recording 12" return
  `None` so the LLM auto-title runs instead). Opus is unsupported (no
  symphonia decoder).
- `MeetingManager::import_recording` reuses the finalize machinery: Gemini
  batch when Gemini is the meeting model (pieces ≤ 50 min, diarized only when
  the whole file fits the 30-min limit), else the local final model over
  `chunk_for_finalize` windows. The whole file is one source labelled
  `others`. Title and summary run inline, so the meeting opens complete.
- An import and a live meeting exclude each other (`importing` flag checked
  under the `state` lock); `is_active()` covers both so the engine stays put.
- Nothing is saved when transcription fails — the user still has the file.
- Commands: `import_meeting_recording`, `cancel_meeting_import`,
  `get_meeting_import_progress`, `get_supported_import_extensions`. Events:
  `meeting-import-progress`, `meeting-import-finished`. UI:
  `meeting/ImportRecording.tsx` (under the idle hero and atop History; file
  picker + window drag-and-drop).

**Re-transcription**: `MeetingManager::retranscribe_meeting(id)` runs the
same pipeline over a completed meeting's saved audio and replaces its
transcript in place (notes/title kept; summary regenerated when it had one;
a datetime placeholder title gets the LLM title). It shares the import's
exclusive slot (`run_exclusive`), progress/finished events and cancel. UI: the
`RetranscribePanel` in `MeetingDetail.tsx`, highlighted when the transcript
looks failed (empty, or < ~30 chars per minute). Interrupted (`recording`)
rows still go through Recover, which has the per-source buffers.

**Audio is MP3** (`audio_toolkit/mp3.rs`, LAME via `mp3lame-encoder`): the
saved playback copy at 48 kbps (`{id}.mp3`, ~22 MB/h) and every cloud upload
(OpenRouter `input_audio` format `mp3`, Gemini Files API `audio/mpeg`) at
64 kbps. Older 32-bit float `{id}.wav` files are converted once in the
background at startup (`convert_wav_audio_to_mp3`); deleting a meeting now
deletes its audio file too.

**Export**: `src-tauri/src/meeting/export.rs` renders Markdown with YAML front
matter (`fisilti_id`). With `meeting_export_dir` set, `MeetingManager::
export_markdown(id)` rewrites the meeting's file whenever it completes or its
title/summary/notes change; the previous file is found by date prefix +
`fisilti_id` and removed after a rename. Only `completed` rows are exported.

## Settings Information Architecture

The sidebar has a fixed set of always-visible sections, grouped into three
clusters (`SECTIONS_CONFIG` in `src/components/Sidebar.tsx`):

| Cluster   | Sections                                     |
| --------- | -------------------------------------------- |
| (unnamed) | General, Models                              |
| Features  | Post-processing, Meetings, Dictation history |
| System    | Advanced, About                              |

**Rules to keep it from drifting back:**

- **One home per setting.** A setting belongs to exactly one page. If it feels
  like it belongs to two, the page split is wrong — don't duplicate it.
- **A feature's on/off switch lives at the top of that feature's own page**,
  never on another page. Sections must not appear/disappear from the sidebar
  based on a toggle.
- **Model-specific settings** (recognition language, translate-to-English,
  cloud API key) live in `models/ActiveModelPanel.tsx`, next to the model
  picker — not on General.
- **Debug/low-level settings** are the "Developer" group at the bottom of
  Advanced, shown only when `debug_mode` is on.
- Long lists use `ui/CollapsibleGroup.tsx` so a page opens showing only what
  is likely needed.
- **One set of settings primitives.** Anything that reads as a setting is built
  from `SettingsGroup` + `SettingContainer` / `ToggleSwitch` / `Dropdown`, on
  every page including the ones inside a feature workspace. Do not grow a
  local toggle or select widget for one page — a toggle must look and behave
  the same everywhere.
- **"History" means dictation history.** The sidebar entry is _Dictation
  history_ (`sidebar.history`); the meeting archive is the History tab inside
  Meetings. Two archives, two names, no shared label.

The models list (`models/ModelsSettings.tsx`) renders ~20 entries as compact
`ModelRow`s bucketed into Installed / Cloud / Multilingual / English only /
Specific languages. Only the first non-empty bucket is expanded. The search box
matches names, descriptions **and** supported languages, which is why there is
no separate language-filter dropdown.

**Model selection is two settings, one page.** `selected_model` drives
dictation; `meeting_selected_model` drives meetings and is empty by default,
meaning "follow the dictation model". Never read `selected_model` from a
meeting code path — go through `AppSettings::meeting_model_id()`, or the two
silently diverge (the loader warms one engine while metadata lookups describe
another). Both pickers live on the Models page: the big list sets the
dictation model, `models/MeetingModelPanel.tsx` sets the meeting one.

Only ONE engine is resident at a time, so `initiate_model_load_for` swaps
whenever the resident model is not the requested one, and the dictation
entry point `initiate_model_load` no-ops entirely while a meeting is running —
the meeting owns the engine for its duration.

Cloud engines are `EngineType::is_cloud()`: OpenRouter (chat + ASR, keyed by
`post_process_api_keys["openrouter"]`) and Gemini (direct to Google, keyed by
`gemini_api_key`). Adding a cloud engine means adding it to `is_cloud()` — that
one predicate drives download status, deletion, local paths, idle-unload
exemption and tray grouping. On the frontend use `lib/utils/model.ts`
(`isCloudModel` / `cloudProviderOf`), never an inline `engine_type ===` check.

**Choosing Gemini as the meeting model is the only switch for Gemini
transcription.** There is deliberately no "finalize with Gemini" toggle: it
used to exist alongside the model choice, which meant two Gemini paths of
different quality picked by a hidden boolean.
`MeetingManager::gemini_finalize_config` keys off the meeting model's engine
type and routes finalize through the per-source `finalize_via_gemini` (which
keeps you/others labels and can diarize) rather than the generic single-blob
cloud path. `meeting_gemini_finalize` survives in `AppSettings` only so
`migrate_gemini_finalize_to_meeting_model` can read it off existing installs;
nothing else may read it.

API keys have one home each: the **API keys** panel on the Models page
(`models/CloudKeysPanel.tsx`). No other page grows a key field — it links to
Models instead, via `emit("navigate-section", "models")` (`App.tsx` listens for
it). Meetings does this because Gemini Live needs the key even when the meeting
model is local; AI editing does it because Google and OpenRouter bill the same
account as the transcription engines.

Two credentials, two storage locations, and the mapping is
`lib/utils/model.ts`'s `keyHomeForProvider` on the frontend:

| Key        | Stored in                             | Post-processing provider |
| ---------- | ------------------------------------- | ------------------------ |
| OpenRouter | `post_process_api_keys["openrouter"]` | `openrouter`             |
| Gemini     | `gemini_api_key` (top level)          | `google`                 |

The Google provider deliberately has **no** entry in `post_process_api_keys`.
Reads go through `AppSettings::post_process_key_for`, which returns
`gemini_api_key` for `GOOGLE_PROVIDER_ID` and ignores the map; writes are
redirected the same way in `change_post_process_api_key_setting`; and
`migrate_google_post_process_key_into_gemini_key` folds away entries left by
older builds. Two copies of one credential means whichever was edited last
silently decides whether the feature works, with nothing on screen saying which
won — do not reintroduce a per-provider Google key "for separate billing".

Every key field anywhere is `PostProcessingSettingsApi/ApiKeyField` — a
password input with draft-then-commit-on-blur. Do not hand-roll a second key
control (an earlier `CloudKeysPanel` mixed a bound field with a write-only one
that showed its state in the placeholder; they read as unrelated widgets).

Meetings is the one sidebar entry that is a **workspace** rather than a
settings page, so it keeps its own Session / History / Settings tabs. Its
Settings tab (`meeting/MeetingPreferences.tsx`) is still built from the shared
primitives: a visible "General" group (shortcut, auto-summarize, calendar
names) plus two `CollapsibleGroup`s — Gemini and automatic detection — that
open only when they are already in use (`geminiInUse`, `autoDetect`). Those
defaults are read from the backend, which is why the tab renders a spacer
until the settings load instead of painting a collapsed group and expanding it
a frame later.

## Debug Mode

Access debug features: `Cmd+Shift+D` (macOS) or `Ctrl+Shift+D` (Windows/Linux).
This reveals the "Developer" group at the bottom of Settings → Advanced
(there is no separate Debug section) and navigates there.

## Platform Notes

- **macOS**: Metal acceleration, accessibility permissions required
- **Windows**: Vulkan acceleration, code signing
- **Linux**: OpenBLAS + Vulkan, limited Wayland support, overlay disabled by default
