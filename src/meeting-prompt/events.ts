// Event names shared with the Rust `meeting_prompt` module. Keep these in sync
// with `MEETING_PROMPT_UPDATE_EVENT` / `MEETING_PROMPT_READY_EVENT` there.

/** Backend → page: carries the `MeetingPromptPayload` to render. */
export const MEETING_PROMPT_UPDATE_EVENT = "meeting-prompt-update";

/** Page → backend: emitted once on mount so the backend replays the payload. */
export const MEETING_PROMPT_READY_EVENT = "meeting-prompt-ready";
