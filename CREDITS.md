# Credits & Attribution

**Fisilti is a fork of [Handy](https://github.com/cjpais/handy) by cjpais, used
under the MIT License.**

The dictation functionality, audio pipeline, model management, and the overall
Tauri + Rust + React application structure originate from Handy. Fisilti extends
it with a meeting mode (system-audio capture, live transcript, speaker labels,
AI summaries, and persistent history) and rebrands the product. We are grateful
to cjpais and the Handy contributors for making the project open source and
forkable.

- Upstream project: https://github.com/cjpais/handy
- Original author: cjpais
- Original license: MIT (preserved in [LICENSE](LICENSE))

## Notable upstream components

These libraries and models are used (directly or transitively) and deserve
acknowledgement:

- **[Tauri](https://tauri.app/)** — the Rust-based desktop app framework
- **[whisper.cpp / ggml](https://github.com/ggml-org/whisper.cpp)** — local
  Whisper inference and GPU acceleration
- **Whisper** by OpenAI — the underlying speech-recognition model
- **[transcribe-rs](https://crates.io/crates/transcribe-rs)** — Rust
  transcription bindings (Whisper / Parakeet, with Metal acceleration on macOS)
- **Parakeet** — CPU-optimized speech-recognition model with automatic language
  detection
- **[Silero VAD](https://github.com/snakers4/silero-vad)** via
  [`vad-rs`](https://github.com/cjpais/vad-rs) — lightweight voice activity
  detection
- **[cidre](https://github.com/yury/cidre)** — Rust bindings to Apple
  frameworks, used for the macOS CoreAudio system-audio tap in meeting mode
- **[cpal](https://crates.io/crates/cpal)** — cross-platform audio I/O
- **[rubato](https://crates.io/crates/rubato)** — audio resampling

Each of these projects is distributed under its own license; please refer to
the respective repositories for details.
