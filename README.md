# Fisilti

**Private, local-first dictation and AI meeting notes for macOS.**

<!-- TODO: add screenshot / logo here -->
<!-- ![Fisilti](docs/screenshot.png) -->

## What it is

Fisilti (technical id: `fisilti`) is a privacy-first desktop app for your voice.
It does two things:

1. **Dictation** — push-to-talk speech-to-text. Press a shortcut, speak, and
   your words are pasted into whatever app you're using.
2. **Meeting mode** — capture a full conversation (your mic + the system audio),
   watch a live transcript, then get an AI-generated summary when you stop.

**Local by default, cloud only if you ask.** Out of the box, speech recognition
runs entirely on your machine and no audio leaves it. Every cloud feature below
is off until you turn it on and paste your own API key, and the app tells you
which one is active: the badge above the transcript reads "100% on-device
transcription" only while that is actually true, and names the service
otherwise.

Cloud options exist because some things genuinely cannot be done on-device
today — telling several remote participants apart, or translating a meeting as
it happens. Whether that trade is worth making is your call, not a default.

## Features

### Dictation (inherited from Handy)

- Push-to-talk / toggle speech-to-text pasted into any text field
- Local transcription via **Whisper** (Small/Medium/Turbo/Large, GPU-accelerated
  when available) or **Parakeet** (CPU-optimized, automatic language detection)
- Voice Activity Detection (Silero VAD) to trim silence
- Optional AI post-processing to clean up transcripts (any provider below)

### Meeting mode

- Captures **microphone + system audio** together using the macOS CoreAudio tap
- **Live transcript** as the meeting runs, with an oscilloscope level meter
- **Per-source labels** ("you" vs. "others") from the separate mic and
  system-audio streams
- **Hybrid high-quality finalize** — a higher-accuracy transcription pass over
  the full audio when you stop, replacing the rough live preview
- **AI meeting-notes summary** via OpenRouter, Google Gemini, Ollama, or any
  OpenAI-compatible provider
- **Persistent meeting history** with stored audio and **playback**
- **Automatic meeting detection** (opt-in) — when a meeting app (Zoom, Teams,
  Webex, WhatsApp, …) or a browser (Google Meet & co. run in tabs) starts using
  the microphone, a small prompt offers to start transcribing
- **Automatic naming** — sessions are titled after the calendar event in
  progress, or the meeting app's window title, instead of a datetime
- **Auto-end on silence** — if nobody speaks for a configurable duration (or
  the meeting app releases the microphone), Fisilti asks whether to end the
  meeting and ends it automatically if the prompt goes unanswered
- **Crash recovery** — a meeting interrupted by a crash, or one whose
  transcription failed, keeps its audio and can be re-transcribed later.
  Nothing is discarded just because a transcription pass came back empty
- Tray menu shortcuts: start/stop a meeting and open the past-meetings list
  without opening the main window

### Cloud transcription with Gemini (opt-in)

All of these are off by default and need a Gemini API key, pasted once in
Meeting → Settings.

- **Speaker identification** — the on-stop finalize pass can run through
  `gemini-3.5-transcribe`, which attributes speech to individual participants
  ("Speaker 1", "Speaker 2", …) instead of one undifferentiated "others" block.
  This is the only option that can tell people apart. Recordings longer than
  30 minutes fall back to plain transcription, which is the API's limit for
  speaker attribution.
- **Live transcription** — `gemini-3.5-transcribe-live` streams text as people
  speak, for a more accurate live transcript than the local model can manage in
  real time.
- **Live translation** — `gemini-3.5-live-translate` shows the original and a
  translation of each line, so you can follow a meeting in a language you don't
  speak.
- **Live subtitles** — a click-through strip near the bottom of your screen
  showing what the other participants are saying, so you can read along without
  leaving the call. Excluded from screen sharing and recordings.
- **Custom vocabulary** — names, products and jargon the model should expect.
- **Cleaned-up transcripts** — drop filler words and false starts instead of
  transcribing every "um" verbatim. (Mutually exclusive with speaker
  identification; the API does not offer both at once.)

**Cost.** Each meeting shows an estimate next to its duration, computed from
the token counts the API reports. Rough per hour of audio: speaker
identification ~$0.30, live transcription ~$0.55, live translation ~$2.20 —
translation is the expensive one because its output is speech. The figure is an
estimate, not an invoice: prices for these preview models change, and free-tier
quota and billing discounts aren't visible to the app.

## Requirements

- **macOS 14.4 or later** — required for system-audio capture (the CoreAudio
  process tap used by meeting mode).
- Microphone and accessibility permissions (granted on first launch).
- Optional: a **calendar** permission, only if you turn on naming meetings after
  the calendar event in progress.
- Optional: a **Gemini API key**, only for the cloud features listed above.

## Models

Transcription models are **downloaded on first use** — there is nothing to
configure manually to get started. Pick a model in Settings and Fisilti fetches
it the first time it's needed. AI summary models are provided by whichever
external provider you configure (and are optional).

Cloud transcription models need no download — they run on the provider's side
and need only an API key.

## Build from source

**Prerequisites:** [Rust](https://rustup.rs/) (latest stable),
[Bun](https://bun.sh/), and the standard
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform
(Xcode command line tools on macOS).

```bash
# Install JS dependencies
bun install

# Build the app (ad-hoc signed — fine for a one-off build)
bun run tauri build

# RECOMMENDED for repeated local builds: sign with a stable identity so macOS
# permissions survive rebuilds (see "Stable signing" below)
bun run build:mac
```

### Stable signing (recommended)

`bun run tauri build` signs the app **ad-hoc**, which gives every build a
different code-signing identity — macOS then treats each rebuild as a new app
and silently drops its permission grants (Accessibility is the most visible
victim: the checkbox looks enabled but no longer applies).

`bun run build:mac` instead signs with a certificate named
**"Fisilti Dev Signing"** from your login keychain, so permissions are granted
once and persist across rebuilds. Create the certificate once (no Apple
account needed):

1. Open **Keychain Access** → menu **Keychain Access → Certificate Assistant →
   Create a Certificate…**
2. Name: `Fisilti Dev Signing` — Identity Type: _Self-Signed Root_ —
   Certificate Type: **Code Signing** → Create.
3. Build with `bun run build:mac`. On the first build macOS asks to allow
   `codesign` to use the key — choose **Always Allow**.

If you switch from ad-hoc builds, reset the stale permission entries once:
`tccutil reset All com.umitaltintas.fisilti`, then re-grant on next launch.

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

> **Note:** with the default ad-hoc build (`bun run tauri build`) the signature
> changes every time you rebuild, so macOS treats a freshly rebuilt copy as a
> new app and the permissions above must be re-granted. Build with
> `bun run build:mac` instead (see [Stable signing](#stable-signing-recommended))
> and permissions persist across rebuilds.

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
