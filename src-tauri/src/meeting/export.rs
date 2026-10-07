// Meetings as Markdown: the document behind the "Export" button, and the
// optional automatic copy into a folder of the user's choosing (an Obsidian
// vault, a notes repo) via the `meeting_export_dir` setting.

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::meeting::manager::TranscriptSource;
use crate::meeting::store::{MeetingStore, STATUS_COMPLETED};
use crate::meeting::MeetingRecord;

/// Render a `MeetingRecord` as a Markdown document: YAML front matter (so
/// Obsidian and friends can query it), then title, date/time, duration, user
/// notes, AI summary and the labeled transcript. Pure/testable.
pub fn render_meeting_markdown(record: &MeetingRecord) -> String {
    use chrono::{DateTime, Local};
    use std::fmt::Write as _;

    let started =
        DateTime::from_timestamp_millis(record.started_at).map(|d| d.with_timezone(&Local));
    let total_secs = (record.duration_ms / 1000).max(0);

    let mut out = String::new();
    out.push_str("---\n");
    let _ = writeln!(out, "title: {}", yaml_string(record.title.trim()));
    if let Some(dt) = started {
        let _ = writeln!(out, "date: {}", dt.format("%Y-%m-%dT%H:%M:%S%:z"));
    }
    let _ = writeln!(out, "duration_minutes: {}", (total_secs + 30) / 60);
    out.push_str("tags: [meeting]\n");
    out.push_str("source: fisilti\n");
    let _ = writeln!(out, "{}{}", ID_KEY, record.id);
    out.push_str("---\n\n");

    let _ = writeln!(out, "# {}", record.title.trim());
    out.push('\n');

    if let Some(local) = started {
        let _ = writeln!(out, "- **Date:** {}", local.format("%B %e, %Y"));
        let _ = writeln!(out, "- **Time:** {}", local.format("%l:%M %p"));
    }
    let _ = writeln!(
        out,
        "- **Duration:** {}h {}m {}s",
        total_secs / 3600,
        (total_secs % 3600) / 60,
        total_secs % 60
    );
    out.push('\n');

    if let Some(notes) = record.notes.as_deref() {
        if !notes.trim().is_empty() {
            let _ = writeln!(out, "## Notes\n\n{}\n", notes.trim());
        }
    }

    if let Some(summary) = record.summary.as_deref() {
        if !summary.trim().is_empty() {
            let _ = writeln!(out, "## Summary\n\n{}\n", summary.trim());
        }
    }

    let _ = writeln!(out, "## Transcript\n");
    if record.segments.is_empty() {
        // No per-segment labels available; emit the raw transcript.
        let _ = writeln!(out, "{}", record.transcript.trim());
    } else {
        let mut ordered: Vec<&_> = record.segments.iter().collect();
        ordered.sort_by_key(|s| s.timestamp_ms);
        for seg in ordered {
            let text = seg.text.trim();
            if text.is_empty() {
                continue;
            }
            // A resolved speaker is strictly more informative than "Others",
            // so it wins when the finalize pass managed to attribute the line.
            let label = match (&seg.speaker, seg.source) {
                (Some(speaker), _) if !speaker.trim().is_empty() => speaker.trim(),
                (_, TranscriptSource::Mic) => "You",
                (_, TranscriptSource::System) => "Others",
            };
            let ts = seg.timestamp_ms / 1000;
            // Hours appear only when needed; a two-hour conference would
            // otherwise read "[95:12]".
            let stamp = if ts >= 3600 {
                format!("{}:{:02}:{:02}", ts / 3600, (ts % 3600) / 60, ts % 60)
            } else {
                format!("{:02}:{:02}", ts / 60, ts % 60)
            };
            let _ = writeln!(out, "- **[{}] {}:** {}", stamp, label, text);
        }
    }

    out
}

/// Front-matter key that ties an exported file back to its meeting row, so a
/// re-export (new title, fresh summary) replaces the file instead of leaving
/// a stale copy next to it.
const ID_KEY: &str = "fisilti_id: ";

/// A double-quoted YAML scalar. Line breaks and other control characters are
/// escaped too: a raw newline inside the quotes would end the front matter
/// line and corrupt every key after it.
fn yaml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Keep meeting `id`'s copy in the export folder current, when the user set
/// one. Called whenever a completed meeting's exported content changes (it
/// finished, got its title or summary). Best-effort: a missing folder or a
/// write error is logged, never surfaced — the meeting itself is safe in the
/// database either way.
pub fn sync_to_export_dir(app: &AppHandle, store: &MeetingStore, id: i64) {
    let dir = crate::settings::get_settings(app).meeting_export_dir;
    let dir = dir.trim();
    if dir.is_empty() {
        return;
    }
    let record = match store.get_meeting(id) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("meeting export: cannot load meeting {}: {}", id, e);
            return;
        }
    };
    // An interrupted or still-finalizing row has a partial transcript; it is
    // exported once it completes.
    if record.status != STATUS_COMPLETED {
        return;
    }
    match write_export(Path::new(dir), &record) {
        Ok(path) => log::info!("meeting export: wrote {:?}", path),
        Err(e) => log::warn!("meeting export: failed for meeting {}: {}", id, e),
    }
}

fn write_export(dir: &Path, record: &MeetingRecord) -> std::io::Result<PathBuf> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("export folder {:?} does not exist", dir),
        ));
    }
    let prefix = file_prefix(record.started_at);
    let target = dir.join(export_file_name(&prefix, &record.title));
    let previous = find_previous_export(dir, &prefix, record.id);

    // Unique per write: two exports of the same meeting can run at once (the
    // auto-title and the auto-summary land on different threads), and a
    // shared temp name let one rename the other's half-written file.
    let tmp = dir.join(unique_tmp_name(record.id));
    if let Err(e) = std::fs::write(&tmp, render_meeting_markdown(record))
        .and_then(|()| std::fs::rename(&tmp, &target))
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some(previous) = previous.filter(|p| *p != target) {
        let _ = std::fs::remove_file(previous);
    }
    Ok(target)
}

/// A temp file name no concurrent writer shares.
fn unique_tmp_name(id: i64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        ".fisilti-{}-{}-{}.md.tmp",
        id,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Remove meeting `record`'s file from the export folder, when one is
/// configured and the file is there. Called when the meeting is deleted, so
/// the vault does not keep a note for a meeting that no longer exists.
pub fn remove_from_export_dir(app: &AppHandle, record: &MeetingRecord) {
    let dir = crate::settings::get_settings(app).meeting_export_dir;
    let dir = dir.trim();
    if dir.is_empty() {
        return;
    }
    remove_export(Path::new(dir), record);
}

fn remove_export(dir: &Path, record: &MeetingRecord) {
    let prefix = file_prefix(record.started_at);
    if let Some(path) = find_previous_export(dir, &prefix, record.id) {
        match std::fs::remove_file(&path) {
            Ok(()) => log::info!("meeting export: removed {:?}", path),
            Err(e) => log::warn!("meeting export: could not remove {:?}: {}", path, e),
        }
    }
}

/// `2026-10-06 2153`: sorts chronologically and never changes for a meeting,
/// which is what lets `find_previous_export` look only at matching names.
fn file_prefix(started_at_ms: i64) -> String {
    use chrono::{DateTime, Local};
    DateTime::from_timestamp_millis(started_at_ms)
        .map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H%M").to_string())
        .unwrap_or_else(|| "meeting".to_string())
}

/// File name for an export: the date prefix plus the title, stripped of
/// characters that are illegal in file names or that Obsidian refuses in note
/// names (`#^[]|`).
fn export_file_name(prefix: &str, title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned: String = cleaned.chars().take(80).collect();
    let cleaned = cleaned.trim().trim_start_matches('.');
    if cleaned.is_empty() {
        format!("{}.md", prefix)
    } else {
        format!("{} {}.md", prefix, cleaned)
    }
}

/// An earlier export of meeting `id` in `dir`, found by its front-matter id
/// among files sharing the meeting's date prefix.
fn find_previous_export(dir: &Path, prefix: &str, id: i64) -> Option<PathBuf> {
    let marker = format!("\n{}{}\n", ID_KEY, id);
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "md")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix))
        })
        .find(|p| {
            std::fs::read_to_string(p)
                .map(|body| front_matter(&body).contains(&marker))
                .unwrap_or(false)
        })
}

/// The front-matter block of `body` including its delimiters, or "".
fn front_matter(body: &str) -> &str {
    if !body.starts_with("---\n") {
        return "";
    }
    match body[4..].find("\n---") {
        Some(end) => &body[..4 + end + 1],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::manager::TranscriptSegment;

    fn record_with(segments: Vec<TranscriptSegment>, transcript: &str) -> MeetingRecord {
        MeetingRecord {
            id: 7,
            started_at: 0,
            ended_at: 65_000,
            duration_ms: 65_000,
            title: "Weekly Sync".to_string(),
            transcript: transcript.to_string(),
            segments,
            summary: Some("AI summary text".to_string()),
            created_at: 0,
            audio_path: None,
            notes: Some("My own notes".to_string()),
            status: "completed".to_string(),
            usage: None,
        }
    }

    fn segment(text: &str, timestamp_ms: u64, source: TranscriptSource) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            timestamp_ms,
            source,
            translation: None,
            speaker: None,
        }
    }

    #[test]
    fn markdown_includes_title_notes_summary_and_labeled_transcript() {
        let segments = vec![
            segment("Hello team.", 0, TranscriptSource::Mic),
            segment("Hi there.", 5000, TranscriptSource::System),
        ];
        let md = render_meeting_markdown(&record_with(segments, "Hello team. Hi there."));
        assert!(md.contains("\n# Weekly Sync"));
        assert!(md.contains("## Notes"));
        assert!(md.contains("My own notes"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("AI summary text"));
        assert!(md.contains("## Transcript"));
        assert!(md.contains("You:** Hello team."));
        assert!(md.contains("Others:** Hi there."));
        // Duration line present (65s -> 0h 1m 5s).
        assert!(md.contains("0h 1m 5s"));
    }

    #[test]
    fn markdown_falls_back_to_raw_transcript_without_segments() {
        let md = render_meeting_markdown(&record_with(Vec::new(), "raw transcript body"));
        assert!(md.contains("## Transcript"));
        assert!(md.contains("raw transcript body"));
    }

    #[test]
    fn markdown_opens_with_front_matter_carrying_the_meeting_id() {
        let mut record = record_with(Vec::new(), "x");
        record.title = "Say \"hi\"".to_string();
        let md = render_meeting_markdown(&record);
        let fm = front_matter(&md);
        assert!(md.starts_with("---\n"));
        assert!(fm.contains("title: \"Say \\\"hi\\\"\"\n"));
        assert!(fm.contains("\nfisilti_id: 7\n"));
        assert!(fm.contains("tags: [meeting]"));
    }

    #[test]
    fn timestamps_past_an_hour_show_hours() {
        let segments = vec![segment("Late remark.", 3_725_000, TranscriptSource::System)];
        let md = render_meeting_markdown(&record_with(segments, "Late remark."));
        assert!(md.contains("[1:02:05] Others:** Late remark."));
    }

    #[test]
    fn file_names_drop_characters_obsidian_rejects() {
        assert_eq!(
            export_file_name("2026-10-06 0915", "Q4: plan / #budget [draft]"),
            "2026-10-06 0915 Q4 plan budget draft.md"
        );
        assert_eq!(export_file_name("p", "  "), "p.md");
    }

    #[test]
    fn re_export_replaces_the_previous_file_after_a_rename() {
        let dir = std::env::temp_dir().join(format!("fisilti-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut record = record_with(Vec::new(), "body");
        record.title = "Meeting 1".to_string();
        let first = write_export(&dir, &record).unwrap();
        record.title = "Real Title".to_string();
        let second = write_export(&dir, &record).unwrap();

        assert_ne!(first, second);
        assert!(!first.exists());
        assert!(second.exists());
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn front_matter_survives_a_title_with_line_breaks() {
        let mut record = record_with(Vec::new(), "x");
        record.title = "Line one\nkey: injected\r\tend".to_string();
        let md = render_meeting_markdown(&record);
        let fm = front_matter(&md);
        assert!(fm.contains("title: \"Line one\\nkey: injected\\r\\tend\"\n"));
        // The id line must still be found — it is how re-exports match.
        assert!(fm.contains("\nfisilti_id: 7\n"));
        assert!(!fm.contains("\nkey: injected"));
    }

    #[test]
    fn concurrent_writers_never_share_a_temp_file() {
        assert_ne!(unique_tmp_name(7), unique_tmp_name(7));
    }

    #[test]
    fn deleting_a_meeting_removes_its_exported_note() {
        let dir = std::env::temp_dir().join(format!(
            "fisilti-export-remove-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let record = record_with(Vec::new(), "body");
        let mut other = record_with(Vec::new(), "body");
        other.id = 8;
        other.title = "Other".to_string();
        let mine = write_export(&dir, &record).unwrap();
        let theirs = write_export(&dir, &other).unwrap();

        remove_export(&dir, &record);
        assert!(!mine.exists());
        assert!(theirs.exists(), "only the deleted meeting's note goes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
