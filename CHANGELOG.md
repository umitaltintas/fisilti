# Changelog

All notable changes to Fısıltı are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-06-01

Initial Fısıltı release. Fısıltı is a fork of
[Handy](https://github.com/cjpais/handy) (MIT) — see [CREDITS.md](CREDITS.md).

### Added

- **Meeting mode** — a new continuous meeting workflow alongside the existing
  push-to-talk dictation:
  - Captures and mixes **microphone + system audio** via the macOS CoreAudio tap
  - **Live transcript** updated as the meeting runs
  - **Oscilloscope / level visualizer** for real-time audio feedback
  - **Per-speaker labels** distinguishing "you" from "others"
  - **Hybrid high-quality finalize** — higher-accuracy transcription pass on stop
  - **AI meeting-notes summary** via OpenRouter, Ollama, or any
    OpenAI-compatible provider
  - **Persistent meeting history** with stored audio and playback

### Changed

- **Rebranded from Handy to Fısıltı** (display name "Fısıltı", id `fisilti`),
  reframed as a privacy-first, local-first dictation and AI meeting-notes app
  for macOS.

[0.1.0]: https://github.com/cjpais/handy
