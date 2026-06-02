# Fisilti

**Private, local-first dictation and AI meeting notes for macOS.**

<!-- TODO: add screenshot / logo here -->
<!-- ![Fisilti](docs/screenshot.png) -->

## What it is

Fisilti (technical id: `fisilti`) is a privacy-first desktop app that keeps your
voice on your own machine. Nothing is sent to the cloud for transcription —
speech recognition runs 100% locally on your computer. It does two things:

1. **Dictation** — push-to-talk speech-to-text. Press a shortcut, speak, and
   your words are pasted into whatever app you're using.
2. **Meeting mode** — capture a full conversation (your mic + the system audio),
   watch a live transcript, then get an AI-generated summary when you stop.

Optional AI summaries can use a provider of your choice (OpenRouter, a local
Ollama instance, or any OpenAI-compatible endpoint), so you stay in control of
where — if anywhere — your data goes.

## Features

### Dictation (inherited from Handy)

- Push-to-talk / toggle speech-to-text pasted into any text field
- Local transcription via **Whisper** (Small/Medium/Turbo/Large, GPU-accelerated
  when available) or **Parakeet** (CPU-optimized, automatic language detection)
- Voice Activity Detection (Silero VAD) to trim silence

### Meeting mode

- Captures **microphone + system audio** together using the macOS CoreAudio tap
- **Live transcript** as the meeting runs
- **Oscilloscope / level visualizer** for real-time audio feedback
- **Per-speaker labels** ("you" vs. "others") from the separate mic and
  system-audio streams
- **Hybrid high-quality finalize** — a higher-accuracy transcription pass when
  you stop the meeting
- **AI meeting-notes summary** via OpenRouter, Ollama, or any OpenAI-compatible
  provider
- **Persistent meeting history** with stored audio and **playback**

## Requirements

- **macOS 14.4 or later** — required for system-audio capture (the CoreAudio
  process tap used by meeting mode).
- Microphone and accessibility permissions (granted on first launch).

## Models

Transcription models are **downloaded on first use** — there is nothing to
configure manually to get started. Pick a model in Settings and Fisilti fetches
it the first time it's needed. AI summary models are provided by whichever
external provider you configure (and are optional).

## Build from source

**Prerequisites:** [Rust](https://rustup.rs/) (latest stable),
[Bun](https://bun.sh/), and the standard
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform
(Xcode command line tools on macOS).

```bash
# Install JS dependencies
bun install

# Build the app
bun run tauri build
```

> **macOS tip:** if the build fails with a CMake policy error, prefix the
> command with `CMAKE_POLICY_VERSION_MINIMUM=3.5`:
>
> ```bash
> CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build
> ```

For day-to-day development use `bun run tauri dev` (same `CMAKE_POLICY_VERSION_MINIMUM`
tip applies).

## Install (macOS)

There is no notarized release yet, so you install the app you build yourself
(see [Build from source](#build-from-source) above). A successful
`bun run tauri build` produces the app bundle at:

```
src-tauri/target/release/bundle/macos/Fısıltı.app
```

**1. Move it to your Applications folder** so it behaves like a normal app
(launchable from Spotlight, Launchpad, and the Dock):

```bash
# from the repo root, after building
ditto "src-tauri/target/release/bundle/macos/Fısıltı.app" "/Applications/Fısıltı.app"
```

(`ditto` preserves the code signature; a plain Finder drag-and-drop into
`Applications` works too.)

**2. First launch — Gatekeeper.** The build is ad-hoc signed (not notarized by
Apple), so macOS may refuse to open it the first time ("Fısıltı can't be opened
because Apple cannot check it for malicious software"). Either:

- **Right-click** the app in `Applications` → **Open** → **Open** in the dialog
  (only needed once), or
- clear the quarantine flag from the terminal:

  ```bash
  xattr -dr com.apple.quarantine "/Applications/Fısıltı.app"
  ```

**3. Grant permissions on first run.** macOS will prompt for these the first
time each is needed — approve them in **System Settings → Privacy & Security**:

- **Accessibility** — required to paste dictated text and use the global
  shortcut.
- **Microphone** — required for dictation and meeting mode.
- **Screen & System Audio Recording** — required for meeting mode's
  system-audio capture (the CoreAudio process tap). Without it, the "others"
  side of a meeting records silence.

> **Note:** because the build is ad-hoc signed, its signature changes every time
> you rebuild, so macOS treats a freshly rebuilt copy as a new app and the
> permissions above must be re-granted. Once installed and granted, the app
> keeps its permissions until you replace it with a new build.

## Built on Handy

Fisilti is a fork / derivative work of
[**Handy**](https://github.com/cjpais/handy) by cjpais, used under the MIT
License. The dictation engine, audio pipeline, and Tauri + Rust + React
foundation come from Handy; Fisilti adds meeting mode and rebrands the product.
Huge thanks to cjpais and the Handy community. See [CREDITS.md](CREDITS.md) for
full attribution.

## License

MIT — see [LICENSE](LICENSE). The original Handy copyright notice is retained as
required by the MIT License.
