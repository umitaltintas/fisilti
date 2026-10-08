// Meeting mode: continuous meeting sessions.
//
// `MeetingManager` (manager.rs) owns the meeting slot and the lifecycle of a
// session (session.rs). Capture runs in capture.rs, the on-stop finalize pass
// in finalize.rs, Gemini Live streaming in live.rs, imports / recovery /
// re-transcription in exclusive.rs, and the LLM summary + title in
// summarize.rs.
//
// It is ADDITIVE and ISOLATED from the dictation flow.

mod buffers;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod dsp;
mod exclusive;
pub mod export;
#[cfg(target_os = "macos")]
mod finalize;
pub mod import;
#[cfg(target_os = "macos")]
mod live;
pub mod manager;
pub mod session;
pub mod store;
pub mod summarize;
#[cfg(target_os = "macos")]
mod text;

pub use manager::{MeetingImportProgress, MeetingManager};
pub use session::{MeetingSessionInfo, MeetingState, StopMeetingResult};
pub use store::{InterruptedMeeting, MeetingListItem, MeetingRecord};
