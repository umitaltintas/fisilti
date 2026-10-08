// Meeting mode persistence layer.
//
// Opens the SAME `history.db` used by `HistoryManager` (the `meetings` table is
// created by an appended migration in `managers::history`). This module is
// ADDITIVE and ISOLATED: it never touches the dictation `transcription_history`
// table or the `HistoryManager` itself, it only reads/writes the `meetings`
// table on its own connections.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use tauri::AppHandle;

use crate::meeting::manager::TranscriptSegment;

/// Lifecycle status of a meeting row.
///
/// `Recording` is written on `start()` (the row exists immediately so a crash
/// mid-meeting doesn't lose data). `Completed` is written on a clean
/// `stop()`/finalize. Rows still at `Recording` at the next app startup are
/// interrupted sessions offered for recovery.
pub const STATUS_RECORDING: &str = "recording";
pub const STATUS_COMPLETED: &str = "completed";

/// A meeting record to persist. `id`/`created_at` are assigned by the store.
#[derive(Clone, Debug)]
pub struct MeetingRecordInput {
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    pub title: String,
    pub transcript: String,
    pub segments: Vec<TranscriptSegment>,
    pub summary: Option<String>,
    /// Absolute path to the persisted mixed 16 kHz mono WAV, if saved.
    pub audio_path: Option<String>,
}

/// Lightweight row for the meetings list view (newest-first).
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingListItem {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    pub title: String,
    pub has_summary: bool,
    /// Short preview of the transcript (first ~200 chars).
    pub transcript_preview: String,
    /// Lifecycle status: `"recording"` (interrupted/in-progress) or
    /// `"completed"`. The list view can surface a "needs recovery" badge.
    pub status: String,
}

/// Full meeting record returned by `get_meeting`.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingRecord {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    pub title: String,
    pub transcript: String,
    pub segments: Vec<TranscriptSegment>,
    pub summary: Option<String>,
    pub created_at: i64,
    /// Absolute path to the persisted mixed 16 kHz mono WAV, if saved.
    pub audio_path: Option<String>,
    /// User's own editable notes, distinct from the AI `summary`.
    pub notes: Option<String>,
    /// Lifecycle status: `"recording"` or `"completed"`.
    pub status: String,
    /// Tokens the Gemini paths reported for this meeting. `None` for meetings
    /// that used no cloud model, or that predate usage tracking — which is why
    /// the UI must say "no data" rather than "$0.00".
    pub usage: Option<crate::ai_usage::MeetingUsage>,
}

/// An interrupted meeting (status still `"recording"`) detected at startup, with
/// the info a recovery pass needs.
#[derive(Clone, Debug, Serialize, Type)]
pub struct InterruptedMeeting {
    pub id: i64,
    pub started_at: i64,
    pub title: String,
    /// Whatever partial transcript was incrementally saved before the crash.
    pub transcript: String,
    /// True if the per-source temp audio buffers still exist on disk (so a
    /// re-finalize can recover a high-quality transcript). False → only the
    /// partial transcript can be salvaged.
    pub has_buffers: bool,
}

/// The raw temp-buffer paths recorded on a row during capture (for recovery).
#[derive(Clone, Debug)]
pub struct StoredBuffers {
    pub mic: Option<String>,
    pub system: Option<String>,
    pub mixed: Option<String>,
}

/// Persistence handle for meeting sessions. Resolves the same `history.db` path
/// as `HistoryManager` and opens a fresh connection per operation.
#[derive(Clone)]
pub struct MeetingStore {
    /// `Err` when the app data directory could not be resolved: every
    /// operation then fails with that reason instead of quietly creating a
    /// stray `history.db` in whatever the working directory happens to be.
    db_path: std::result::Result<PathBuf, String>,
}

const PREVIEW_LEN: usize = 200;

fn make_preview(transcript: &str) -> String {
    let trimmed = transcript.trim();
    if trimmed.chars().count() <= PREVIEW_LEN {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(PREVIEW_LEN).collect();
        format!("{}…", truncated)
    }
}

impl MeetingStore {
    /// Construct a store pointing at `{app_data_dir}/history.db` (the same file
    /// `HistoryManager` uses). The `meetings` table is created by the appended
    /// migration that `HistoryManager::new` runs at startup, so the DB is
    /// expected to already exist and be migrated by the time this is used.
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let db_path = app_data_dir.join("history.db");
        Ok(Self {
            db_path: Ok(db_path),
        })
    }

    /// Construct a store from an explicit db path (tests).
    #[cfg(test)]
    pub fn with_db_path(db_path: PathBuf) -> Self {
        Self {
            db_path: Ok(db_path),
        }
    }

    /// A store that refuses every operation with `reason`. Used when the app
    /// data dir cannot be resolved, so the failure surfaces on use.
    pub fn unavailable(reason: String) -> Self {
        Self {
            db_path: Err(reason),
        }
    }

    fn get_connection(&self) -> Result<Connection> {
        match &self.db_path {
            Ok(path) => Ok(Connection::open(path)?),
            Err(reason) => Err(anyhow::anyhow!("meeting store unavailable: {}", reason)),
        }
    }

    /// Insert a meeting record. Returns the new row id.
    pub fn save_meeting(&self, record: &MeetingRecordInput) -> Result<i64> {
        let created_at = chrono::Utc::now().timestamp_millis();
        let segments_json = serde_json::to_string(&record.segments)?;

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO meetings (
                started_at,
                ended_at,
                duration_ms,
                title,
                transcript,
                segments_json,
                summary,
                created_at,
                audio_path,
                status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.started_at,
                record.ended_at,
                record.duration_ms,
                &record.title,
                &record.transcript,
                &segments_json,
                &record.summary,
                created_at,
                &record.audio_path,
                STATUS_COMPLETED,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// CRASH-RECOVERY: insert an in-progress meeting row at the START of a
    /// session, with status `recording`, an initial title, and the per-source
    /// temp-buffer paths the capture loop streams audio to. Returns the new id.
    /// Transcript starts empty and is filled incrementally via
    /// `update_in_progress`.
    pub fn start_meeting(
        &self,
        started_at: i64,
        title: &str,
        buffers: &StoredBuffers,
    ) -> Result<i64> {
        let created_at = chrono::Utc::now().timestamp_millis();
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO meetings (
                started_at,
                ended_at,
                duration_ms,
                title,
                transcript,
                segments_json,
                summary,
                created_at,
                audio_path,
                status,
                buffer_mic_path,
                buffer_system_path,
                buffer_mixed_path
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                started_at,
                started_at, // ended_at == started_at until finalized
                0i64,
                title,
                "",   // transcript filled incrementally
                "[]", // segments_json
                Option::<String>::None,
                created_at,
                Option::<String>::None,
                STATUS_RECORDING,
                &buffers.mic,
                &buffers.system,
                &buffers.mixed,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// CRASH-RECOVERY: incrementally persist the current transcript + segments of
    /// an in-progress session. Called periodically by the capture loop (batched).
    /// Cheap single-row UPDATE; never changes status.
    pub fn update_in_progress(
        &self,
        id: i64,
        transcript: &str,
        segments: &[TranscriptSegment],
        ended_at: i64,
        duration_ms: i64,
    ) -> Result<()> {
        let segments_json = serde_json::to_string(segments)?;
        let conn = self.get_connection()?;
        // Only while the row is still recording: a late incremental write
        // landing after finalize must not replace the final transcript with
        // the live preview.
        let changed = conn.execute(
            "UPDATE meetings
             SET transcript = ?1, segments_json = ?2, ended_at = ?3, duration_ms = ?4
             WHERE id = ?5 AND status = ?6",
            params![
                transcript,
                &segments_json,
                ended_at,
                duration_ms,
                id,
                STATUS_RECORDING
            ],
        )?;
        expect_one_row(changed, id)
    }

    /// Keep the live clock of an in-progress row current: bumps only
    /// `ended_at`/`duration_ms`, leaving the transcript untouched. Called on a
    /// timer by the capture loop so a running meeting reports its real length
    /// even when no transcript segments are being produced (e.g. a cloud model,
    /// where the live pass is skipped entirely).
    pub fn update_progress_timestamp(
        &self,
        id: i64,
        ended_at: i64,
        duration_ms: i64,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        let changed = conn.execute(
            "UPDATE meetings SET ended_at = ?1, duration_ms = ?2 WHERE id = ?3 AND status = ?4",
            params![ended_at, duration_ms, id, STATUS_RECORDING],
        )?;
        expect_one_row(changed, id)
    }

    /// Record the token usage a meeting accumulated. Separate from
    /// `finalize_meeting` because the recovery path finalizes a row whose usage
    /// was spent in an earlier process, and overwriting it with the current
    /// session's zero would erase what the first attempt actually cost.
    pub fn update_usage(&self, id: i64, usage_json: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE meetings SET usage_json = ?2 WHERE id = ?1",
            params![id, usage_json],
        )?;
        Ok(())
    }

    /// CRASH-RECOVERY: finalize a row to `completed`, writing the final
    /// transcript/segments/timestamps and clearing the temp-buffer paths. Used
    /// by the normal stop() path, recovery and re-transcription. Never touches
    /// `notes`, `title` or `summary`: the user may have typed notes into the
    /// row while it was recording. Fails when the row no longer exists (it
    /// was deleted meanwhile) instead of reporting a save that never happened.
    pub fn finalize_meeting(
        &self,
        id: i64,
        transcript: &str,
        segments: &[TranscriptSegment],
        ended_at: i64,
        duration_ms: i64,
    ) -> Result<()> {
        let segments_json = serde_json::to_string(segments)?;
        let conn = self.get_connection()?;
        let changed = conn.execute(
            "UPDATE meetings
             SET transcript = ?1,
                 segments_json = ?2,
                 ended_at = ?3,
                 duration_ms = ?4,
                 status = ?5,
                 buffer_mic_path = NULL,
                 buffer_system_path = NULL,
                 buffer_mixed_path = NULL
             WHERE id = ?6",
            params![
                transcript,
                &segments_json,
                ended_at,
                duration_ms,
                STATUS_COMPLETED,
                id
            ],
        )?;
        expect_one_row(changed, id)
    }

    /// Update the title column of an existing meeting row.
    pub fn update_title(&self, id: i64, title: &str) -> Result<()> {
        let conn = self.get_connection()?;
        let changed = conn.execute(
            "UPDATE meetings SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        expect_one_row(changed, id)
    }

    /// Update the user notes column of an existing meeting row.
    pub fn update_notes(&self, id: i64, notes: &str) -> Result<()> {
        let conn = self.get_connection()?;
        let changed = conn.execute(
            "UPDATE meetings SET notes = ?1 WHERE id = ?2",
            params![notes, id],
        )?;
        expect_one_row(changed, id)
    }

    /// Update the summary column of an existing meeting row.
    pub fn update_summary(&self, id: i64, summary: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE meetings SET summary = ?1 WHERE id = ?2",
            params![summary, id],
        )?;
        Ok(())
    }

    /// Update the audio_path column of an existing meeting row.
    pub fn update_audio_path(&self, id: i64, audio_path: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE meetings SET audio_path = ?1 WHERE id = ?2",
            params![audio_path, id],
        )?;
        Ok(())
    }

    /// Fetch the absolute audio path for a meeting, if one was saved.
    pub fn get_audio_path(&self, id: i64) -> Result<Option<String>> {
        let conn = self.get_connection()?;
        let path = conn
            .query_row(
                "SELECT audio_path FROM meetings WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>("audio_path"),
            )
            .optional()?
            .flatten();
        Ok(path)
    }

    /// Fetch the recorded temp-buffer paths for a meeting (for recovery).
    /// Meetings whose saved playback audio is still a WAV file, oldest first.
    /// Feeds the one-off conversion to MP3.
    pub fn list_wav_audio(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, audio_path FROM meetings
             WHERE audio_path LIKE '%.wav' ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every capture-buffer path any row still records (rows keep them only
    /// while `recording`). Used to sweep buffers nothing refers to.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn referenced_buffer_paths(&self) -> Result<std::collections::HashSet<String>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT buffer_mic_path, buffer_system_path, buffer_mixed_path FROM meetings
             WHERE buffer_mic_path IS NOT NULL
                OR buffer_system_path IS NOT NULL
                OR buffer_mixed_path IS NOT NULL",
        )?;
        let mut paths = std::collections::HashSet::new();
        let rows = stmt.query_map([], |row| {
            Ok([
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ])
        })?;
        for row in rows {
            paths.extend(row?.into_iter().flatten());
        }
        Ok(paths)
    }

    pub fn get_buffers(&self, id: i64) -> Result<StoredBuffers> {
        let conn = self.get_connection()?;
        let buffers = conn
            .query_row(
                "SELECT buffer_mic_path, buffer_system_path, buffer_mixed_path
                 FROM meetings WHERE id = ?1",
                params![id],
                |row| {
                    Ok(StoredBuffers {
                        mic: row.get("buffer_mic_path")?,
                        system: row.get("buffer_system_path")?,
                        mixed: row.get("buffer_mixed_path")?,
                    })
                },
            )
            .optional()?
            .unwrap_or(StoredBuffers {
                mic: None,
                system: None,
                mixed: None,
            });
        Ok(buffers)
    }

    /// List meetings of EVERY status, newest-first. When `query` is `Some`,
    /// filters by a case-insensitive substring match against title,
    /// transcript, or summary (SQLite `LIKE`). `None` → all meetings.
    ///
    /// Rows still in `recording` status (live, finalizing, or interrupted) are
    /// included with their `status`, so a session that takes minutes to
    /// finalize never looks lost; the UI marks them by status.
    pub fn list_meetings(&self, query: Option<&str>) -> Result<Vec<MeetingListItem>> {
        let conn = self.get_connection()?;
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MeetingListItem> {
            let transcript: String = row.get("transcript")?;
            let summary: Option<String> = row.get("summary")?;
            Ok(MeetingListItem {
                id: row.get("id")?,
                started_at: row.get("started_at")?,
                ended_at: row.get("ended_at")?,
                duration_ms: row.get("duration_ms")?,
                title: row.get("title")?,
                has_summary: summary
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false),
                transcript_preview: make_preview(&transcript),
                status: row.get("status")?,
            })
        };

        let items = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => {
                // Case-insensitive LIKE. Escape the LIKE wildcards in the user's
                // query so '%' / '_' are treated literally.
                let escaped = q
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                let pattern = format!("%{}%", escaped);
                let mut stmt = conn.prepare(
                    "SELECT id, started_at, ended_at, duration_ms, title, transcript, summary, status
                     FROM meetings
                     WHERE title LIKE ?1 ESCAPE '\\'
                        OR transcript LIKE ?1 ESCAPE '\\'
                        OR IFNULL(summary, '') LIKE ?1 ESCAPE '\\' 
                     ORDER BY started_at DESC, id DESC",
                )?;
                let rows: Vec<MeetingListItem> = stmt
                    .query_map(params![pattern], map_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, started_at, ended_at, duration_ms, title, transcript, summary, status
                     FROM meetings
                     ORDER BY started_at DESC, id DESC",
                )?;
                let rows: Vec<MeetingListItem> = stmt
                    .query_map([], map_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            }
        };
        Ok(items)
    }

    /// List meetings left in `recording` status (interrupted by a crash/kill),
    /// newest-first. Checks whether their temp audio buffers still exist on disk.
    ///
    /// `live` is the row of the session that is running or finalizing right
    /// now: it is in `recording` status too, but it is not interrupted, and
    /// offering to recover or discard it would destroy the meeting in progress.
    pub fn list_interrupted(&self, live: Option<i64>) -> Result<Vec<InterruptedMeeting>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, started_at, title, transcript, buffer_mic_path, buffer_system_path
             FROM meetings
             WHERE status = ?1 AND id != ?2
             ORDER BY started_at DESC, id DESC",
        )?;
        let items = stmt
            .query_map(params![STATUS_RECORDING, live.unwrap_or(-1)], |row| {
                let mic: Option<String> = row.get("buffer_mic_path")?;
                let system: Option<String> = row.get("buffer_system_path")?;
                let has_buffers = [mic, system]
                    .iter()
                    .flatten()
                    .any(|p| std::path::Path::new(p).exists());
                Ok(InterruptedMeeting {
                    id: row.get("id")?,
                    started_at: row.get("started_at")?,
                    title: row.get("title")?,
                    transcript: row.get("transcript")?,
                    has_buffers,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(items)
    }

    /// Fetch a single full meeting record by id.
    pub fn get_meeting(&self, id: i64) -> Result<MeetingRecord> {
        let conn = self.get_connection()?;
        let record = conn
            .query_row(
                "SELECT id, started_at, ended_at, duration_ms, title, transcript, segments_json, summary, created_at, audio_path, notes, status, usage_json
                 FROM meetings WHERE id = ?1",
                params![id],
                |row| {
                    let segments_json: String = row.get("segments_json")?;
                    Ok((
                        MeetingRecord {
                            id: row.get("id")?,
                            started_at: row.get("started_at")?,
                            ended_at: row.get("ended_at")?,
                            duration_ms: row.get("duration_ms")?,
                            title: row.get("title")?,
                            transcript: row.get("transcript")?,
                            segments: Vec::new(),
                            summary: row.get("summary")?,
                            created_at: row.get("created_at")?,
                            audio_path: row.get("audio_path")?,
                            notes: row.get("notes")?,
                            status: row.get("status")?,
                            // Usage is decorative: a malformed blob must not
                            // stop a meeting from opening.
                            usage: row
                                .get::<_, Option<String>>("usage_json")?
                                .and_then(|j| serde_json::from_str(&j).ok()),
                        },
                        segments_json,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Meeting {} not found", id))?;

        let (mut record, segments_json) = record;
        record.segments = serde_json::from_str(&segments_json).unwrap_or_default();
        Ok(record)
    }

    /// Delete a meeting row by id.
    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        Ok(())
    }
}

/// An UPDATE that matched nothing means the row is gone (deleted while we
/// worked) or no longer in the expected state. Reporting success there is how
/// a meeting's final transcript used to vanish without a trace.
fn expect_one_row(changed: usize, id: i64) -> Result<()> {
    if changed == 0 {
        Err(anyhow::anyhow!(
            "Meeting {} not found (or no longer in the expected state)",
            id
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite_migration::Migrations;

    /// A store backed by a fresh temp database carrying the REAL schema, so
    /// these tests break if a migration changes the `meetings` table.
    fn temp_store(name: &str) -> MeetingStore {
        let path = std::env::temp_dir().join(format!("fisilti_store_test_{}.db", name));
        let _ = std::fs::remove_file(&path);
        let mut conn = Connection::open(&path).expect("open temp db");
        Migrations::new(crate::managers::history::MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .expect("migrate temp db");
        MeetingStore::with_db_path(path)
    }

    fn buffers() -> StoredBuffers {
        StoredBuffers {
            mic: Some("/tmp/mic.f32".into()),
            system: Some("/tmp/sys.f32".into()),
            mixed: Some("/tmp/mix.f32".into()),
        }
    }

    #[test]
    fn list_meetings_reports_in_progress_rows_with_their_status() {
        let store = temp_store("in_progress_status");
        let running = store
            .start_meeting(1_000, "Running meeting", &buffers())
            .expect("insert in-progress");
        let done = store
            .start_meeting(2_000, "Finished meeting", &buffers())
            .expect("insert second");
        store
            .finalize_meeting(done, "hello there", &[], 5_000, 3_000)
            .expect("finalize");

        // A meeting that is still recording or still finalizing must remain
        // visible — hiding it makes a session that takes minutes to transcribe
        // look like it was lost. The status is what lets the UI mark it.
        let all = store.list_meetings(None).expect("list");
        let by_id = |id: i64| all.iter().find(|m| m.id == id).expect("row present");
        assert_eq!(by_id(running).status, "recording");
        assert_eq!(by_id(done).status, "completed");

        // The same must hold for the search path, not just the unfiltered list.
        let searched = store.list_meetings(Some("meeting")).expect("search");
        let mut ids: Vec<i64> = searched.iter().map(|m| m.id).collect();
        ids.sort();
        assert_eq!(ids, vec![running, done]);
    }

    #[test]
    fn progress_clock_updates_duration_without_touching_transcript() {
        let store = temp_store("progress_clock");
        let id = store
            .start_meeting(1_000, "Live meeting", &buffers())
            .expect("insert");
        store
            .update_in_progress(id, "partial text", &[], 4_000, 3_000)
            .expect("incremental persist");

        store
            .update_progress_timestamp(id, 61_000, 60_000)
            .expect("progress clock");

        let record = store.get_meeting(id).expect("get");
        assert_eq!(record.duration_ms, 60_000);
        assert_eq!(record.ended_at, 61_000);
        assert_eq!(
            record.transcript, "partial text",
            "the clock update must not clobber the transcript"
        );
        assert_eq!(record.status, STATUS_RECORDING);
    }

    #[test]
    fn the_live_meeting_is_never_offered_for_recovery() {
        let store = temp_store("interrupted_excludes_live");
        let crashed = store
            .start_meeting(1_000, "Crashed earlier", &buffers())
            .expect("insert crashed");
        let live = store
            .start_meeting(2_000, "Running now", &buffers())
            .expect("insert live");

        let ids = |live: Option<i64>| -> Vec<i64> {
            store
                .list_interrupted(live)
                .expect("list")
                .iter()
                .map(|m| m.id)
                .collect()
        };
        assert_eq!(ids(Some(live)), vec![crashed]);
        // With nothing running, both are genuinely interrupted.
        assert_eq!(ids(None), vec![live, crashed]);
    }

    #[test]
    fn finalizing_a_deleted_meeting_is_an_error_not_a_silent_success() {
        let store = temp_store("finalize_missing");
        let id = store
            .start_meeting(1_000, "Doomed", &buffers())
            .expect("insert");
        store.delete_meeting(id).expect("delete");
        assert!(store
            .finalize_meeting(id, "text", &[], 2_000, 1_000)
            .is_err());
        assert!(store
            .update_in_progress(id, "text", &[], 2_000, 1_000)
            .is_err());
    }

    #[test]
    fn a_late_incremental_write_cannot_clobber_the_final_transcript() {
        let store = temp_store("late_incremental");
        let id = store
            .start_meeting(1_000, "Meeting", &buffers())
            .expect("insert");
        store
            .finalize_meeting(id, "final text", &[], 5_000, 4_000)
            .expect("finalize");
        assert!(store
            .update_in_progress(id, "live preview", &[], 6_000, 5_000)
            .is_err());
        assert!(store.update_progress_timestamp(id, 9_000, 8_000).is_err());
        let record = store.get_meeting(id).expect("get");
        assert_eq!(record.transcript, "final text");
        assert_eq!(record.duration_ms, 4_000);
    }

    #[test]
    fn notes_typed_during_recording_survive_finalize() {
        let store = temp_store("notes_survive");
        let id = store
            .start_meeting(1_000, "Meeting", &buffers())
            .expect("insert");
        store.update_notes(id, "my live notes").expect("notes");
        store
            .update_in_progress(id, "partial", &[], 2_000, 1_000)
            .expect("incremental");
        store
            .finalize_meeting(id, "final", &[], 3_000, 2_000)
            .expect("finalize");
        let record = store.get_meeting(id).expect("get");
        assert_eq!(record.notes.as_deref(), Some("my live notes"));
        assert_eq!(record.transcript, "final");
    }

    #[test]
    fn only_rows_still_holding_buffers_are_referenced() {
        let store = temp_store("referenced_buffers");
        let live = store
            .start_meeting(1_000, "Live", &buffers())
            .expect("insert");
        let paths = store.referenced_buffer_paths().expect("paths");
        assert!(paths.contains("/tmp/mic.f32"));
        assert!(paths.contains("/tmp/mix.f32"));
        store
            .finalize_meeting(live, "done", &[], 2_000, 1_000)
            .expect("finalize");
        assert!(store.referenced_buffer_paths().expect("paths").is_empty());
    }

    #[test]
    fn an_unavailable_store_fails_instead_of_creating_a_stray_database() {
        let store = MeetingStore::unavailable("no app dir".to_string());
        let err = store.list_meetings(None).unwrap_err().to_string();
        assert!(err.contains("no app dir"), "{err}");
    }

    #[test]
    fn in_progress_row_starts_at_zero_duration() {
        // Guards the premise of the progress clock: without an update, a running
        // meeting really does read as a zero-second one.
        let store = temp_store("zero_duration");
        let id = store
            .start_meeting(1_000, "Live meeting", &buffers())
            .expect("insert");
        let record = store.get_meeting(id).expect("get");
        assert_eq!(record.duration_ms, 0);
        assert_eq!(record.ended_at, record.started_at);
    }
}
