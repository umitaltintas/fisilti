import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import { listen } from "@tauri-apps/api/event";
import i18n from "@/i18n";
import { errorMessage } from "@/lib/utils/errors";
import {
  MEETING_EVENTS,
  deleteMeeting as deleteMeetingCommand,
  discardInterruptedMeeting,
  getMeeting,
  getMeetingSession,
  getMeetingTranscript,
  listInterruptedMeetings,
  listMeetings,
  recoverMeeting,
  regenerateMeetingSummary,
  startMeeting,
  stopMeeting,
  summarizeMeetingWith,
  toMeetingStatus,
  updateMeetingNotes,
  type InterruptedMeeting,
  type MeetingFinalizing,
  type MeetingImportFinished,
  type MeetingListItem,
  type MeetingSessionInfo,
  type MeetingStatus,
  type MeetingSummaryUpdate,
  type MeetingTitleUpdate,
  type MeetingTranscriptUpdate,
  type TranscriptSegment,
} from "@/lib/meeting";

export const NOTES_AUTOSAVE_MS = 800;
export const SEARCH_DEBOUNCE_MS = 300;

/**
 * Where the live notes stand relative to the database. Shown verbatim next to
 * the notes, so it must never claim "saved" before the backend said so.
 *
 * - `idle`: nothing typed since the session (or the store) started.
 * - `pending`: typed, but the meeting row does not exist yet (the session is
 *   starting, or this session saved nothing) — not persisted.
 * - `saving`: a save is scheduled or in flight.
 * - `saved`: the last edit is persisted.
 * - `error`: the last save failed; `notesError` says why.
 */
export type NotesSaveState = "idle" | "pending" | "saving" | "saved" | "error";

export type LoadState = "loading" | "ready" | "error";

interface MeetingStore {
  initialized: boolean;

  // The live session — or, once it has ended, the session that just finished,
  // kept on screen until the next one starts.
  status: MeetingStatus;
  /** Row id of the live / just-finished meeting, once the backend knows it. */
  meetingId: number | null;
  startedAtMs: number | null;
  transcript: string;
  segments: TranscriptSegment[];
  sessionTitle: string | null;
  /** Localized, user-facing error about the session. */
  error: string | null;
  /** A start or stop request is in flight. */
  busy: boolean;

  notes: string;
  notesSave: NotesSaveState;
  notesError: string | null;

  summary: string;
  summarizing: boolean;
  summaryError: string | null;

  // The archive.
  meetings: MeetingListItem[];
  meetingsState: LoadState;
  meetingsError: string | null;
  query: string;
  interrupted: InterruptedMeeting[];

  initialize: () => Promise<void>;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  setNotes: (notes: string) => void;
  summarize: (template?: string) => Promise<void>;
  dismissError: () => void;
  setQuery: (query: string) => void;
  loadMeetings: () => Promise<void>;
  loadInterrupted: () => Promise<void>;
  /** Delete a saved meeting. Rejects with the backend error. */
  deleteMeeting: (id: number) => Promise<void>;
  /** Recover an interrupted meeting. Rejects with the backend error. */
  recover: (id: number) => Promise<void>;
  /** Discard an interrupted meeting. Rejects with the backend error. */
  discard: (id: number) => Promise<void>;
  /** Merge a renamed / re-summarized meeting into the list. */
  patchMeeting: (id: number, patch: Partial<MeetingListItem>) => void;
}

const t = (key: string, options?: Record<string, unknown>) =>
  i18n.t(key, options);

// Module-level bookkeeping that must not trigger renders.
let initPromise: Promise<void> | null = null;
let notesTimer: ReturnType<typeof setTimeout> | null = null;
let notesSeq = 0;
let searchTimer: ReturnType<typeof setTimeout> | null = null;
let listSeq = 0;

const freshSession = {
  meetingId: null,
  startedAtMs: null,
  transcript: "",
  segments: [] as TranscriptSegment[],
  sessionTitle: null,
  error: null,
  notes: "",
  notesSave: "idle" as NotesSaveState,
  notesError: null,
  summary: "",
  summarizing: false,
  summaryError: null,
};

export const useMeetingStore = create<MeetingStore>()(
  subscribeWithSelector((set, get) => {
    // -- notes -------------------------------------------------------------

    const saveNotesNow = async () => {
      if (notesTimer) {
        clearTimeout(notesTimer);
        notesTimer = null;
      }
      const { meetingId: id, notes, notesSave } = get();
      if (id == null || (notesSave !== "saving" && notesSave !== "pending")) {
        return;
      }
      const seq = ++notesSeq;
      set({ notesSave: "saving", notesError: null });
      try {
        await updateMeetingNotes(id, notes);
        // Only report on the newest save for the same meeting; an older
        // request finishing late must not paint over a newer edit's state.
        if (seq === notesSeq && get().meetingId === id) {
          set({
            notesSave: get().notes === notes ? "saved" : "saving",
          });
        }
      } catch (error) {
        if (seq === notesSeq && get().meetingId === id) {
          set({ notesSave: "error", notesError: errorMessage(error) });
        }
      }
    };

    const scheduleNotesSave = (delay = NOTES_AUTOSAVE_MS) => {
      if (notesTimer) clearTimeout(notesTimer);
      notesTimer = setTimeout(() => {
        notesTimer = null;
        void saveNotesNow();
      }, delay);
    };

    // -- session transitions ----------------------------------------------

    /** Pull the saved row for the session that just ended: polished
     * segments, auto-summary, title — and notes, if none were typed here. */
    const adoptRecord = async (id: number) => {
      try {
        const record = await getMeeting(id);
        if (get().meetingId !== id) return;
        const local = get();
        set({
          segments:
            record.segments.length > 0 ? record.segments : local.segments,
          transcript: record.transcript.trim()
            ? record.transcript
            : local.transcript,
          summary: record.summary?.trim() ? record.summary : local.summary,
          sessionTitle: record.title || local.sessionTitle,
          ...(local.notes.trim() === "" && record.notes
            ? { notes: record.notes, notesSave: "saved" as const }
            : {}),
        });
      } catch (error) {
        // Keep the live preview as a fallback.
        console.warn(`Failed to load meeting ${id}:`, error);
      }
    };

    const onSessionEnded = async () => {
      // Notes typed in the last moments still go to the row.
      await saveNotesNow();
      const id = get().meetingId;
      await Promise.all([
        get().loadMeetings(),
        get().loadInterrupted(),
        id != null ? adoptRecord(id) : Promise.resolve(),
      ]);
    };

    const applySession = (info: {
      state: unknown;
      meeting_id?: number | null;
      started_at_ms?: number | null;
    }) => {
      const prev = get();
      const next = toMeetingStatus(info.state);
      const id = info.meeting_id ?? null;

      const startedNew =
        next === "running" &&
        (prev.status === "idle" ||
          (id != null && prev.meetingId != null && id !== prev.meetingId));

      if (startedNew) {
        if (notesTimer) clearTimeout(notesTimer);
        notesTimer = null;
        set({ ...freshSession });
      }

      const current = get();
      const meetingId = id ?? current.meetingId;
      set({
        status: next,
        meetingId,
        startedAtMs:
          info.started_at_ms ??
          (next === "running" && current.startedAtMs == null
            ? Date.now()
            : current.startedAtMs),
      });

      // The row now exists: write notes typed while it was being created.
      if (current.meetingId == null && meetingId != null) {
        if (current.notesSave === "pending") {
          set({ notesSave: "saving" });
          scheduleNotesSave(0);
        }
      }

      if (prev.status !== "idle" && next === "idle") {
        void onSessionEnded();
      }
    };

    /** Re-read the authoritative session snapshot (fills the row id and
     * start time when only a bare state event was received). */
    const refreshSession = async () => {
      try {
        applySession(await getMeetingSession());
      } catch (error) {
        console.warn("Failed to read meeting session:", error);
      }
    };

    return {
      initialized: false,
      status: "idle",
      ...freshSession,
      busy: false,
      meetings: [],
      meetingsState: "loading",
      meetingsError: null,
      query: "",
      interrupted: [],

      initialize: () => {
        if (initPromise) return initPromise;
        initPromise = (async () => {
          // App-lifetime listeners: this store outlives every page, which is
          // the point — navigating away from Meetings must not drop a
          // running session's transcript or notes.
          void listen<MeetingSessionInfo>(MEETING_EVENTS.session, (e) =>
            applySession(e.payload),
          );
          void listen<string>(MEETING_EVENTS.state, (e) => {
            const next = toMeetingStatus(e.payload);
            if (next === get().status) return;
            applySession({ state: next });
            if (next === "running" && get().meetingId == null) {
              void refreshSession();
            }
          });
          void listen<MeetingTranscriptUpdate>(
            MEETING_EVENTS.transcript,
            (e) => {
              set((s) => ({
                transcript: e.payload.full_transcript,
                segments: [...s.segments, e.payload.segment],
              }));
            },
          );
          void listen<MeetingFinalizing>(MEETING_EVENTS.finalizing, (e) => {
            const { status } = get();
            if (e.payload.finalizing) {
              if (status === "running") set({ status: "finalizing" });
            } else if (status === "finalizing") {
              applySession({ state: "idle" });
            } else {
              // Already idle (e.g. the stop command returned first): the
              // finalize pass just wrote the row, so refresh what shows it.
              void get().loadMeetings();
              void get().loadInterrupted();
            }
          });
          void listen<MeetingSummaryUpdate | string>(
            MEETING_EVENTS.summary,
            (e) => {
              const payload: MeetingSummaryUpdate =
                typeof e.payload === "string"
                  ? { id: null, summary: e.payload }
                  : e.payload;
              const { meetingId } = get();
              if (
                payload.id === meetingId ||
                (payload.id === null && meetingId !== null)
              ) {
                set({ summary: payload.summary, summaryError: null });
              }
              const target = payload.id ?? meetingId;
              if (target != null) {
                get().patchMeeting(target, {
                  has_summary: payload.summary.trim().length > 0,
                });
              }
            },
          );
          void listen<MeetingTitleUpdate>(MEETING_EVENTS.title, (e) => {
            const { id, title } = e.payload;
            if (id === get().meetingId) set({ sessionTitle: title });
            get().patchMeeting(id, { title });
          });
          void listen<string>(MEETING_EVENTS.error, (e) => {
            set({
              error: t("meeting.transcriptionFailed", { error: e.payload }),
            });
          });
          void listen<MeetingImportFinished>(
            MEETING_EVENTS.importFinished,
            (e) => {
              if (e.payload.id != null) void get().loadMeetings();
            },
          );

          await refreshSession();
          const { status, meetingId } = get();
          if (status !== "idle") {
            // Attached mid-meeting (window opened late): show what exists.
            const [transcript] = await Promise.all([
              getMeetingTranscript().catch(() => ""),
              meetingId != null ? adoptRecord(meetingId) : Promise.resolve(),
            ]);
            if (!get().transcript) set({ transcript });
          }
          set({ initialized: true });
          await Promise.all([get().loadMeetings(), get().loadInterrupted()]);
        })();
        return initPromise;
      },

      start: async () => {
        set({ busy: true, error: null });
        try {
          await startMeeting();
          // The session event usually lands first and already reset the
          // view; only do it here if it has not.
          if (get().status !== "running") {
            applySession({ state: "running", started_at_ms: Date.now() });
          }
          if (get().meetingId == null) void refreshSession();
        } catch (error) {
          set({
            error: t("meeting.errors.startFailed", {
              error: errorMessage(error),
            }),
          });
        } finally {
          set({ busy: false });
        }
      },

      stop: async () => {
        set({ busy: true, error: null });
        if (get().status === "running") set({ status: "finalizing" });
        // Fire the pending notes save; the backend never overwrites notes on
        // finalize, so it does not have to finish before the stop.
        void saveNotesNow();
        try {
          const result = await stopMeeting();
          set((s) => ({
            meetingId: result.meeting_id ?? s.meetingId,
            transcript: result.transcript.trim()
              ? result.transcript
              : s.transcript,
          }));
          if (get().status !== "idle") {
            applySession({ state: "idle" });
          } else {
            // Ended via an event while we waited; the id may only be known
            // now, so adopt the saved row explicitly.
            void onSessionEnded();
          }
        } catch (error) {
          set({
            error: t("meeting.errors.stopFailed", {
              error: errorMessage(error),
            }),
          });
          await refreshSession();
        } finally {
          set({ busy: false });
        }
      },

      setNotes: (notes) => {
        set({ notes });
        if (get().meetingId == null) {
          set({ notesSave: "pending", notesError: null });
          return;
        }
        set({ notesSave: "saving", notesError: null });
        scheduleNotesSave();
      },

      summarize: async (template) => {
        set({ summarizing: true, summaryError: null });
        try {
          const id = get().meetingId;
          // With a known row, summarize exactly that one; the id-less command
          // targets "the last saved meeting", which may be a different one.
          const summary =
            id != null
              ? await regenerateMeetingSummary(id, template)
              : await summarizeMeetingWith(template);
          set({ summary });
          if (id != null) {
            get().patchMeeting(id, { has_summary: summary.trim().length > 0 });
          } else {
            void get().loadMeetings();
          }
        } catch (error) {
          set({
            summaryError: t("meeting.errors.summaryFailed", {
              error: errorMessage(error),
            }),
          });
        } finally {
          set({ summarizing: false });
        }
      },

      dismissError: () => set({ error: null }),

      setQuery: (query) => {
        set({ query });
        if (searchTimer) clearTimeout(searchTimer);
        searchTimer = setTimeout(() => {
          searchTimer = null;
          void get().loadMeetings();
        }, SEARCH_DEBOUNCE_MS);
      },

      loadMeetings: async () => {
        const seq = ++listSeq;
        const query = get().query;
        if (get().meetingsState === "error") {
          set({ meetingsState: "loading", meetingsError: null });
        }
        try {
          const meetings = await listMeetings(query);
          if (seq !== listSeq) return;
          set({ meetings, meetingsState: "ready", meetingsError: null });
        } catch (error) {
          if (seq !== listSeq) return;
          set({
            meetingsState: "error",
            meetingsError: errorMessage(error),
          });
        }
      },

      loadInterrupted: async () => {
        try {
          const interrupted = await listInterruptedMeetings();
          // The live meeting's row is still "recording" too; never offer to
          // recover or discard the session that is running right now.
          const { status, meetingId } = get();
          set({
            interrupted: interrupted.filter(
              (m) => status === "idle" || m.id !== meetingId,
            ),
          });
        } catch (error) {
          console.warn("Failed to list interrupted meetings:", error);
        }
      },

      deleteMeeting: async (id) => {
        await deleteMeetingCommand(id);
        set((s) => ({ meetings: s.meetings.filter((m) => m.id !== id) }));
        if (get().meetingId === id && get().status === "idle") {
          if (notesTimer) clearTimeout(notesTimer);
          notesTimer = null;
          set({ ...freshSession });
        }
        void get().loadMeetings();
      },

      recover: async (id) => {
        await recoverMeeting(id);
        set((s) => ({
          interrupted: s.interrupted.filter((m) => m.id !== id),
        }));
        await get().loadMeetings();
      },

      discard: async (id) => {
        await discardInterruptedMeeting(id);
        set((s) => ({
          interrupted: s.interrupted.filter((m) => m.id !== id),
        }));
        await get().loadMeetings();
      },

      patchMeeting: (id, patch) =>
        set((s) => ({
          meetings: s.meetings.map((m) =>
            m.id === id ? { ...m, ...patch } : m,
          ),
        })),
    };
  }),
);

/** Whether a meeting row belongs to the session that is live right now. */
export const isLiveMeeting = (
  id: number,
  state: Pick<MeetingStore, "status" | "meetingId">,
): boolean => state.status !== "idle" && state.meetingId === id;
