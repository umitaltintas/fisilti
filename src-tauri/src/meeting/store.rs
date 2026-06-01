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
    db_path: PathBuf,
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
        Ok(Self { db_path })
    }

    /// Construct a store from an explicit db path. Used as a fallback when the
    /// app data dir cannot be resolved.
    pub fn with_db_path(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
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
        conn.execute(
            "UPDATE meetings
             SET transcript = ?1, segments_json = ?2, ended_at = ?3, duration_ms = ?4
             WHERE id = ?5",
            params![transcript, &segments_json, ended_at, duration_ms, id],
        )?;
        Ok(())
    }

    /// CRASH-RECOVERY: finalize an in-progress row to `completed`, writing the
    /// final transcript/segments/timestamps and clearing the temp-buffer paths.
    /// Used by both the normal stop() path and the recovery path.
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
        conn.execute(
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
        Ok(())
    }

    /// Update the title column of an existing meeting row.
    pub fn update_title(&self, id: i64, title: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE meetings SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    /// Update the user notes column of an existing meeting row.
    pub fn update_notes(&self, id: i64, notes: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE meetings SET notes = ?1 WHERE id = ?2",
            params![notes, id],
        )?;
        Ok(())
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

    /// List meetings, newest-first. When `query` is `Some`, filters by a
    /// case-insensitive substring match against title, transcript, or summary
    /// (SQLite `LIKE`). `None` → all meetings (unchanged legacy behavior).
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
    pub fn list_interrupted(&self) -> Result<Vec<InterruptedMeeting>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, started_at, title, transcript, buffer_mic_path, buffer_system_path
             FROM meetings
             WHERE status = ?1
             ORDER BY started_at DESC, id DESC",
        )?;
        let items = stmt
            .query_map(params![STATUS_RECORDING], |row| {
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
                "SELECT id, started_at, ended_at, duration_ms, title, transcript, segments_json, summary, created_at, audio_path, notes, status
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
