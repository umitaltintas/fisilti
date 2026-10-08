// Engine-owning background jobs that run while no meeting is live: importing
// a recording, re-transcribing a saved meeting, and recovering an interrupted
// one. They share one exclusive slot (`importing`), so they never overlap
// each other or a live session. Also: deleting meetings with everything they
// own on disk, and the startup maintenance of saved audio.

use std::sync::atomic::Ordering;
use std::sync::Mutex;

use super::manager::{
    default_meeting_title, join_segments, MeetingImportFinished, MeetingImportProgress,
    MeetingImportStage, MeetingManager,
};
use super::session::MeetingState;
use super::store::{MeetingRecordInput, StoredBuffers, STATUS_COMPLETED, STATUS_RECORDING};

/// Clears the exclusive slot however the job ends — including a panic, which
/// used to leave `importing` set and every later import, recovery and meeting
/// start refused until the app restarted.
struct ExclusiveSlot<'a> {
    manager: &'a MeetingManager,
}

impl Drop for ExclusiveSlot<'_> {
    fn drop(&mut self) {
        if let Ok(mut progress) = self.manager.import_progress.lock() {
            *progress = None;
        }
        self.manager.importing.store(false, Ordering::SeqCst);
    }
}

impl MeetingManager {
    /// Run `job` as THE engine-owning background job: refuses while a meeting
    /// is live (or finalizing) or another job runs.
    fn run_exclusive<T>(&self, job: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        {
            let slot = self.slot.lock().unwrap();
            if slot.state != MeetingState::Idle {
                return Err("Stop the current meeting first.".to_string());
            }
            if self.importing.swap(true, Ordering::SeqCst) {
                return Err("Another recording is already being transcribed.".to_string());
            }
        }
        let _slot = ExclusiveSlot { manager: self };
        self.import_cancel.store(false, Ordering::SeqCst);
        job()
    }

    /// Report how an import / re-transcription ended on
    /// `"meeting-import-finished"`, so a window that did not start it (or was
    /// reopened mid-way) can settle its progress card.
    fn report_import_finished(&self, result: &Result<i64, String>) {
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-import-finished",
            MeetingImportFinished {
                id: result.as_ref().ok().copied(),
                error: result.as_ref().err().cloned(),
            },
        );
    }

    /// Progress of the running import, or `None` when nothing is importing.
    pub fn import_progress(&self) -> Option<MeetingImportProgress> {
        self.import_progress.lock().unwrap().clone()
    }

    /// Ask the running import to stop at its next checkpoint. A no-op when
    /// nothing is importing.
    pub fn cancel_import(&self) {
        if self.importing.load(Ordering::SeqCst) {
            self.import_cancel.store(true, Ordering::SeqCst);
        }
    }

    fn report_import(&self, file_name: &str, stage: MeetingImportStage, progress: Option<f32>) {
        let update = MeetingImportProgress {
            file_name: file_name.to_string(),
            stage,
            progress,
        };
        *self.import_progress.lock().unwrap() = Some(update.clone());
        use tauri::Emitter;
        let _ = self.app_handle.emit("meeting-import-progress", update);
    }

    /// Import a recording made elsewhere (a phone voice memo, a conference
    /// recording) as a completed meeting, and return its row id.
    ///
    /// Runs the same transcription the on-stop finalize pass uses — Gemini
    /// batch when Gemini is the meeting model, the local final model in
    /// windows otherwise — then the usual auto-title and auto-summary, so the
    /// result is a normal meeting in History. Blocking: call it off the main
    /// thread. Nothing is saved when transcription fails; the user still has
    /// the file and can simply try again.
    pub fn import_recording(&self, path: &std::path::Path) -> Result<i64, String> {
        let result = self.run_exclusive(|| {
            #[cfg(target_os = "macos")]
            return self.run_import(path);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = path;
                Err("Importing recordings is only supported on macOS".to_string())
            }
        });
        self.report_import_finished(&result);
        result
    }

    /// Transcribe a saved meeting again from its stored audio and replace its
    /// transcript in place — the way out when a meeting came back empty,
    /// partial or garbled. Notes, title and audio stay; the summary is
    /// regenerated when the meeting had one (or auto-summarize is on), and a
    /// datetime placeholder title gets the LLM title. The saved audio is the
    /// mixed track, so the result is labelled as one source. Blocking.
    pub fn retranscribe_meeting(&self, id: i64) -> Result<i64, String> {
        let result = self.ensure_not_live(id).and_then(|()| {
            self.run_exclusive(|| {
                #[cfg(target_os = "macos")]
                return self.run_retranscribe(id);
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = id;
                    Err("Re-transcription is only supported on macOS".to_string())
                }
            })
        });
        self.report_import_finished(&result);
        result
    }

    /// CRASH-RECOVERY. Recover an interrupted meeting `id` left in `recording`
    /// status. On macOS, if the per-source capture buffers still exist,
    /// re-runs the finalize pass (through Gemini when that is the meeting
    /// model) and writes the playback audio. Otherwise keeps the partial
    /// transcript that was incrementally saved. The row is flipped to
    /// `completed` and the buffers are removed. Returns the transcript.
    ///
    /// Runs in the exclusive slot: refuses while a meeting is live, and the
    /// live meeting's own row is never "recovered" out from under it.
    pub fn recover_meeting(&self, id: i64) -> Result<String, String> {
        self.ensure_not_live(id)?;
        self.run_exclusive(|| self.run_recover(id))
    }

    fn run_recover(&self, id: i64) -> Result<String, String> {
        let record = self
            .store
            .get_meeting(id)
            .map_err(|e| format!("Failed to load meeting {}: {}", id, e))?;
        if record.status != STATUS_RECORDING {
            // Already completed (or recovered concurrently). Nothing to do.
            return Ok(record.transcript);
        }

        // Default outcome: keep the partial transcript/segments already saved.
        #[allow(unused_mut)]
        let mut final_segments = record.segments.clone();
        #[allow(unused_mut)]
        let mut final_transcript = record.transcript.clone();
        let usage = Mutex::new(crate::ai_usage::MeetingUsage::default());

        #[cfg(target_os = "macos")]
        {
            let buffers = self
                .store
                .get_buffers(id)
                .map_err(|e| format!("Failed to read meeting buffers: {}", e))?;

            let gemini = self.gemini_finalize_config();
            if gemini.is_none() {
                let settings = crate::settings::get_settings(&self.app_handle);
                self.transcription_manager
                    .initiate_model_load_for(settings.meeting_model_id());
            }
            let result = self.transcribe_capture_buffers(
                buffers.mic.as_deref().map(std::path::Path::new),
                buffers.system.as_deref().map(std::path::Path::new),
                gemini.as_ref(),
                &usage,
            );

            // Save the playback audio if the mixed buffer survived — before any
            // early return, so the audio is preserved either way.
            if let Some(mixed) = buffers.mixed.as_deref() {
                if let Err(e) = self.save_audio_from_raw(id, std::path::Path::new(mixed)) {
                    log::warn!("meeting recovery: playback audio not saved: {}", e);
                }
            }

            // Every request failed again (still no API balance, still offline).
            // KEEP the buffers and leave the row in `recording` so the user gets
            // another attempt once the cause is fixed.
            if let Some(reason) = result.total_failure() {
                log::warn!(
                    "meeting: recovery of row {} failed ({}); leaving it recoverable",
                    id,
                    reason
                );
                self.emit_error(&reason);
                return Err(reason);
            }
            if result.has_text() {
                final_segments = result.segments;
                final_transcript = join_segments(&final_segments);
            }
        }

        let ended_at = if record.ended_at > record.started_at {
            record.ended_at
        } else {
            super::manager::now_epoch_ms()
        };
        let duration_ms = (ended_at - record.started_at).max(0);
        let buffers = self.store.get_buffers(id).ok();
        self.store
            .finalize_meeting(
                id,
                &final_transcript,
                &final_segments,
                ended_at,
                duration_ms,
            )
            .map_err(|e| format!("Failed to finalize recovered meeting: {}", e))?;
        // The meeting is complete: its capture buffers have served their purpose.
        if let Some(buffers) = buffers {
            remove_stored_buffers(&buffers);
        }
        // The recovery pass may have run real cloud transcription; charge it.
        self.persist_usage(id, &usage);
        self.export_markdown(id);
        log::info!("meeting: recovered interrupted row {} (completed)", id);
        Ok(final_transcript)
    }

    #[cfg(target_os = "macos")]
    fn run_retranscribe(&self, id: i64) -> Result<i64, String> {
        use super::import;

        let record = self
            .store
            .get_meeting(id)
            .map_err(|e| format!("Failed to load meeting {}: {}", id, e))?;
        if record.status != STATUS_COMPLETED {
            return Err(
                "This meeting is still being recovered; use Recover on it instead.".to_string(),
            );
        }
        let audio_path = record
            .audio_path
            .clone()
            .filter(|p| std::path::Path::new(p).exists())
            .ok_or_else(|| "This meeting has no saved audio to transcribe again.".to_string())?;
        let label = if record.title.trim().is_empty() {
            format!("#{}", id)
        } else {
            record.title.trim().to_string()
        };

        self.report_import(&label, MeetingImportStage::Decoding, Some(0.0));
        let decoded = import::decode_file(
            std::path::Path::new(&audio_path),
            &self.import_cancel,
            |p| self.report_import(&label, MeetingImportStage::Decoding, Some(p)),
        )?;

        let usage = Mutex::new(crate::ai_usage::MeetingUsage::default());
        let segments = self.transcribe_import(&decoded.samples, &label, &usage)?;
        drop(decoded);
        if self.import_cancel.load(Ordering::SeqCst) {
            return Err(import::CANCELLED.to_string());
        }
        if !segments.iter().any(|s| !s.text.trim().is_empty()) {
            // Keep what the meeting had rather than replacing it with nothing.
            return Err("No speech was found; the existing transcript was kept.".to_string());
        }

        let transcript = join_segments(&segments);
        self.store
            .finalize_meeting(
                id,
                &transcript,
                &segments,
                record.ended_at,
                record.duration_ms,
            )
            .map_err(|e| format!("Failed to save the new transcript: {}", e))?;
        // Merged into what the meeting already cost, not replacing it.
        self.persist_usage(id, &usage);

        let settings = crate::settings::get_settings(&self.app_handle);
        let placeholder_title = record.title.trim() == default_meeting_title(record.started_at);
        let resummarize = record
            .summary
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || settings.meeting_auto_summarize;
        if placeholder_title || resummarize {
            self.report_import(&label, MeetingImportStage::Summarizing, None);
        }
        if placeholder_title {
            match tauri::async_runtime::block_on(super::summarize::generate_title(
                &self.app_handle,
                &transcript,
            )) {
                Ok(title) if !title.trim().is_empty() => self.apply_title(id, title.trim()),
                Ok(_) => {}
                Err(e) => log::info!("meeting retranscribe: auto-title skipped: {}", e),
            }
        }
        if resummarize {
            // Re-read the notes: the user may have edited them meanwhile.
            let notes = self.store.get_meeting(id).ok().and_then(|r| r.notes);
            match tauri::async_runtime::block_on(super::summarize::summarize_transcript(
                &self.app_handle,
                &transcript,
                notes.as_deref(),
            )) {
                Ok(summary) => self.save_summary(id, &summary),
                // The old summary stays; it is stale but better than nothing.
                Err(e) => log::warn!("meeting retranscribe: summary failed: {}", e),
            }
        }

        self.export_markdown(id);
        log::info!("meeting retranscribe: meeting {} transcribed again", id);
        Ok(id)
    }

    #[cfg(target_os = "macos")]
    fn run_import(&self, path: &std::path::Path) -> Result<i64, String> {
        use super::import;

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        self.report_import(&file_name, MeetingImportStage::Decoding, Some(0.0));
        let decoded = import::decode_file(path, &self.import_cancel, |p| {
            self.report_import(&file_name, MeetingImportStage::Decoding, Some(p))
        })?;
        if decoded.samples.is_empty() {
            return Err("The file contains no audio.".to_string());
        }
        let duration_ms = decoded.duration_ms();
        log::info!(
            "meeting import: decoded {:?} ({} s)",
            file_name,
            duration_ms / 1000
        );

        // The usage tally is per job; an import is one.
        let usage = Mutex::new(crate::ai_usage::MeetingUsage::default());
        let segments = self.transcribe_import(&decoded.samples, &file_name, &usage)?;
        if self.import_cancel.load(Ordering::SeqCst) {
            return Err(import::CANCELLED.to_string());
        }
        if !segments.iter().any(|s| !s.text.trim().is_empty()) {
            return Err("No speech was found in the recording.".to_string());
        }

        let started_at = import::recording_started_at(path, decoded.recorded_at_ms, duration_ms);
        let file_title = import::title_from_path(path);
        let transcript = join_segments(&segments);
        let record = MeetingRecordInput {
            started_at,
            ended_at: started_at + duration_ms,
            duration_ms,
            title: file_title
                .clone()
                .unwrap_or_else(|| default_meeting_title(started_at)),
            transcript: transcript.clone(),
            segments,
            summary: None,
            audio_path: None,
        };
        let id = self
            .store
            .save_meeting(&record)
            .map_err(|e| format!("Failed to save the imported meeting: {}", e))?;
        self.persist_usage(id, &usage);
        if let Err(e) = self.write_meeting_audio(id, &decoded.samples) {
            // The transcript is the point; playback is a nicety.
            log::warn!("meeting import: playback audio not saved: {}", e);
        }
        drop(decoded);

        // Title and summary run inline (not spawned like after a live stop):
        // the import is already a background job the user is waiting on, and
        // this way the meeting opens complete instead of filling in later.
        let settings = crate::settings::get_settings(&self.app_handle);
        if file_title.is_none() || settings.meeting_auto_summarize {
            self.report_import(&file_name, MeetingImportStage::Summarizing, None);
        }
        if file_title.is_none() {
            match tauri::async_runtime::block_on(super::summarize::generate_title(
                &self.app_handle,
                &transcript,
            )) {
                Ok(title) if !title.trim().is_empty() => self.apply_title(id, title.trim()),
                Ok(_) => {}
                Err(e) => log::info!("meeting import: auto-title skipped: {}", e),
            }
        }
        if settings.meeting_auto_summarize {
            match tauri::async_runtime::block_on(super::summarize::summarize_transcript(
                &self.app_handle,
                &transcript,
                None,
            )) {
                Ok(summary) => self.save_summary(id, &summary),
                Err(e) => log::warn!("meeting import: auto-summary failed: {}", e),
            }
        }

        self.export_markdown(id);
        log::info!("meeting import: saved {:?} as row {}", file_name, id);
        Ok(id)
    }

    /// Transcribe an imported recording. The whole file is one source: a
    /// phone on the table cannot tell "you" from "others", so everything is
    /// labelled as the room ("others"), which is also the stream Gemini may
    /// diarize (only when the whole recording fits in one request).
    #[cfg(target_os = "macos")]
    fn transcribe_import(
        &self,
        audio: &[f32],
        file_name: &str,
        usage: &Mutex<crate::ai_usage::MeetingUsage>,
    ) -> Result<Vec<super::manager::TranscriptSegment>, String> {
        use super::buffers::AudioSource;
        use super::finalize::SourceInput;
        use super::manager::TranscriptSource;

        let mut sources = [SourceInput {
            source: TranscriptSource::System,
            audio: AudioSource::Memory(audio),
            label: "import",
        }];
        let cancel = &self.import_cancel;

        let result = if let Some(config) = self.gemini_finalize_config() {
            let result = self.transcribe_with_gemini(&mut sources, &config, usage, &mut |i, n| {
                let progress = (n > 1).then(|| i as f32 / n as f32);
                self.report_import(file_name, MeetingImportStage::Transcribing, progress);
                !cancel.load(Ordering::SeqCst)
            });
            // A piece that failed leaves a hole the user would not notice in
            // an import; fail the whole job instead (nothing is saved, the file
            // is still there to retry).
            if result.errors.count > 0 && !cancel.load(Ordering::SeqCst) {
                return Err(result
                    .errors
                    .first
                    .unwrap_or_else(|| "Gemini transcription failed.".to_string()));
            }
            result
        } else {
            self.report_import(file_name, MeetingImportStage::Transcribing, Some(0.0));
            self.transcription_manager.initiate_model_load_for(
                crate::settings::get_settings(&self.app_handle).meeting_model_id(),
            );
            let result = self.transcribe_locally(&mut sources, &mut |i, n| {
                self.report_import(
                    file_name,
                    MeetingImportStage::Transcribing,
                    Some(i as f32 / n.max(1) as f32),
                );
                !cancel.load(Ordering::SeqCst)
            });
            if let Some(reason) = result.total_failure() {
                return Err(reason);
            }
            result
        };

        if cancel.load(Ordering::SeqCst) {
            return Err(super::import::CANCELLED.to_string());
        }
        let mut segments = result.segments;
        segments.sort_by_key(|s| s.timestamp_ms);
        Ok(segments)
    }

    /// Directory of saved playback audio: `{app_data}/meetings`.
    #[cfg(target_os = "macos")]
    fn audio_dir(&self) -> Result<std::path::PathBuf, String> {
        let dir = crate::portable::app_data_dir(&self.app_handle)
            .map_err(|e| e.to_string())?
            .join("meetings");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    /// Point row `id` at its new playback file and drop any older WAV copy.
    #[cfg(target_os = "macos")]
    fn adopt_audio(&self, id: i64, path: &std::path::Path) -> Result<(), String> {
        self.store
            .update_audio_path(id, &path.to_string_lossy())
            .map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_file(dir.join(format!("{}.wav", id)));
        }
        Ok(())
    }

    /// Write 16 kHz mono `samples` as the playback audio of meeting `id`
    /// (`{app_data_dir}/meetings/{id}.mp3`) and record the path on the row.
    #[cfg(target_os = "macos")]
    fn write_meeting_audio(&self, id: i64, samples: &[f32]) -> Result<std::path::PathBuf, String> {
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
        use crate::audio_toolkit::mp3;
        let path = self.audio_dir()?.join(format!("{}.mp3", id));
        mp3::write_mp3_file(&path, samples, WHISPER_SAMPLE_RATE, mp3::STORAGE_BITRATE)?;
        self.adopt_audio(id, &path)?;
        Ok(path)
    }

    /// Encode a raw capture buffer as the playback audio of meeting `id`,
    /// streaming (a multi-hour buffer is never loaded whole). `Ok(None)` when
    /// the buffer held no audio.
    #[cfg(target_os = "macos")]
    pub(super) fn save_audio_from_raw(
        &self,
        id: i64,
        raw: &std::path::Path,
    ) -> Result<Option<std::path::PathBuf>, String> {
        use crate::audio_toolkit::mp3;
        let path = self.audio_dir()?.join(format!("{}.mp3", id));
        if !super::buffers::write_mp3_from_raw(raw, &path, mp3::STORAGE_BITRATE)? {
            return Ok(None);
        }
        self.adopt_audio(id, &path)?;
        log::info!("meeting: saved playback audio to {:?}", path);
        Ok(Some(path))
    }

    /// Delete meeting `id` and everything only it owns on disk: its capture
    /// buffers (an interrupted row still holds hundreds of MB of them), its
    /// playback audio, and its note in the export folder. Refuses the meeting
    /// that is live or still being saved.
    ///
    /// File removal is best-effort: a missing or unreadable file must not stop
    /// the user from deleting a meeting.
    pub fn delete_meeting(&self, id: i64) -> Result<(), String> {
        self.ensure_not_live(id)?;
        let record = self.store.get_meeting(id).ok();
        let buffers = self.store.get_buffers(id).ok();
        self.store
            .delete_meeting(id)
            .map_err(|e| format!("Failed to delete meeting: {}", e))?;

        let mut removed = 0;
        if let Some(buffers) = &buffers {
            removed += remove_stored_buffers(buffers);
        }
        if let Some(audio) = record.as_ref().and_then(|r| r.audio_path.clone()) {
            match std::fs::remove_file(&audio) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("delete: could not remove audio {:?}: {}", audio, e),
            }
        }
        if let Some(record) = &record {
            super::export::remove_from_export_dir(&self.app_handle, record);
        }
        log::info!("delete: removed meeting {} and {} file(s)", id, removed);
        Ok(())
    }

    /// Startup maintenance, on a background thread: convert meetings still
    /// stored as WAV to MP3, then remove capture buffers no meeting refers to
    /// any more (left by a stop whose audio save failed, or by older builds).
    #[cfg(target_os = "macos")]
    pub fn convert_wav_audio_to_mp3(&self) {
        self.convert_legacy_wavs();
        self.sweep_orphan_buffers();
    }

    /// Convert every meeting still stored as WAV to MP3, one at a time.
    /// Meetings recorded before MP3 storage kept 32-bit float WAVs (~230 MB
    /// per hour). Each file is replaced only after its MP3 is written and the
    /// row points at it, so an interrupted run just resumes next launch.
    #[cfg(target_os = "macos")]
    fn convert_legacy_wavs(&self) {
        let pending = match self.store.list_wav_audio() {
            Ok(rows) => rows,
            Err(e) => {
                log::warn!("audio conversion: cannot list meetings: {}", e);
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        log::info!(
            "audio conversion: {} meeting(s) to convert to MP3",
            pending.len()
        );
        for (id, wav) in pending {
            let wav = std::path::PathBuf::from(wav);
            if !wav.exists() {
                continue;
            }
            // Streamed, not decoded whole: one forgotten session ran for
            // seven hours (1.6 GB of WAV).
            let mp3 = wav.with_extension("mp3");
            use crate::audio_toolkit::mp3;
            if let Err(e) = mp3::transcode_wav_file(&wav, &mp3, mp3::STORAGE_BITRATE) {
                log::warn!("audio conversion: meeting {} failed: {}", id, e);
                continue;
            }
            match self.store.update_audio_path(id, &mp3.to_string_lossy()) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&wav);
                    log::info!("audio conversion: meeting {} -> {:?}", id, mp3);
                }
                Err(e) => {
                    log::warn!("audio conversion: meeting {} not updated: {}", id, e);
                    let _ = std::fs::remove_file(&mp3);
                }
            }
        }
    }

    /// Delete capture buffers in the buffer dir that no `recording` row refers
    /// to. Only files untouched for an hour: a session that just started
    /// writes its buffers before its row exists.
    #[cfg(target_os = "macos")]
    fn sweep_orphan_buffers(&self) {
        const MIN_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);
        let referenced = match self.store.referenced_buffer_paths() {
            Ok(paths) => paths,
            Err(e) => {
                log::warn!("buffer sweep: cannot list meetings: {}", e);
                return;
            }
        };
        let live: Vec<std::path::PathBuf> = self
            .current_session()
            .and_then(|s| s.buffers())
            .map(|b| vec![b.mic, b.system, b.mixed])
            .unwrap_or_default();
        let dir = super::buffers::buffer_dir(&self.app_handle);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
            let is_buffer = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("fisilti_meeting_") && n.ends_with(".f32"));
            if !is_buffer
                || referenced.contains(path.to_string_lossy().as_ref())
                || live.contains(&path)
            {
                continue;
            }
            let old_enough = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age >= MIN_AGE);
            if old_enough {
                match std::fs::remove_file(&path) {
                    Ok(()) => log::info!("buffer sweep: removed orphan {:?}", path),
                    Err(e) => log::warn!("buffer sweep: could not remove {:?}: {}", path, e),
                }
            }
        }
    }
}

/// Remove the capture buffer files a row recorded. Returns how many existed.
fn remove_stored_buffers(buffers: &StoredBuffers) -> usize {
    let mut removed = 0;
    for path in [&buffers.mic, &buffers.system, &buffers.mixed]
        .into_iter()
        .flatten()
    {
        match std::fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("meeting: could not remove buffer {}: {}", path, e),
        }
    }
    removed
}
