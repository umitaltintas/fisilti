# Fısıltı

**Private, local-first dictation and AI meeting notes for macOS.**

<!-- TODO: add screenshot / logo here -->
<!-- ![Fısıltı](docs/screenshot.png) -->

## What it is

Fısıltı (technical id: `fisilti`) is a privacy-first desktop app that keeps your
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
configure manually to get started. Pick a model in Settings and Fısıltı fetches
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

## Built on Handy

Fısıltı is a fork / derivative work of
[**Handy**](https://github.com/cjpais/handy) by cjpais, used under the MIT
License. The dictation engine, audio pipeline, and Tauri + Rust + React
foundation come from Handy; Fısıltı adds meeting mode and rebrands the product.
Huge thanks to cjpais and the Handy community. See [CREDITS.md](CREDITS.md) for
full attribution.

## License

MIT — see [LICENSE](LICENSE). The original Handy copyright notice is retained as
required by the MIT License.
