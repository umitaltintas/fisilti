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
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.started_at,
                record.ended_at,
                record.duration_ms,
                &record.title,
                &record.transcript,
                &segments_json,
                &record.summary,
                created_at,
            ],
        )?;
        Ok(conn.last_insert_rowid())
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

    /// List all meetings, newest-first.
    pub fn list_meetings(&self) -> Result<Vec<MeetingListItem>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, started_at, ended_at, duration_ms, title, transcript, summary
             FROM meetings
             ORDER BY started_at DESC, id DESC",
        )?;
        let items = stmt
            .query_map([], |row| {
                let transcript: String = row.get("transcript")?;
                let summary: Option<String> = row.get("summary")?;
                Ok(MeetingListItem {
                    id: row.get("id")?,
                    started_at: row.get("started_at")?,
                    ended_at: row.get("ended_at")?,
                    duration_ms: row.get("duration_ms")?,
                    title: row.get("title")?,
                    has_summary: summary.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false),
                    transcript_preview: make_preview(&transcript),
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
                "SELECT id, started_at, ended_at, duration_ms, title, transcript, segments_json, summary, created_at
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
