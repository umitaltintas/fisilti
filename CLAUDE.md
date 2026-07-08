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

# Build for LOCAL install (signs with the stable "Fisilti Dev Signing"
# keychain identity so macOS TCC permissions survive rebuilds — always
# prefer this when the build will be installed to /Applications)
bun run build:mac

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

## Debug Mode

Access debug features: `Cmd+Shift+D` (macOS) or `Ctrl+Shift+D` (Windows/Linux)

## Platform Notes

- **macOS**: Metal acceleration, accessibility permissions required
- **Windows**: Vulkan acceleration, code signing
- **Linux**: OpenBLAS + Vulkan, limited Wayland support, overlay disabled by default
