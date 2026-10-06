// Meeting mode (Step 3): continuous meeting session.
//
// This top-level module owns the `MeetingManager`, which runs a long-lived
// session capturing mixed mic + system audio, segmenting it with VAD, and
// transcribing each segment via Fısıltı's existing `TranscriptionManager`.
//
// It is ADDITIVE and ISOLATED from the dictation flow.

pub mod export;
pub mod import;
pub mod manager;
pub mod store;

pub use manager::{MeetingImportProgress, MeetingManager, MeetingState};
pub use store::{InterruptedMeeting, MeetingListItem, MeetingRecord};
