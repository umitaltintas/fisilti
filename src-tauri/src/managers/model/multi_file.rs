//! Directory models published as loose files (e.g. a Hugging Face repo)
//! instead of one `.tar.gz`. Each file is fetched into `{filename}.partial/`
//! with its own resume and checksum, and the directory is renamed into place
//! only once every file is complete, so a half-downloaded model is never
//! mistaken for an installed one.

use super::{
    classify_resume, DownloadCleanup, DownloadProgress, ModelManager, ResumeOutcome,
    DOWNLOAD_STALL_TIMEOUT,
};
use anyhow::Result;
use futures_util::StreamExt;
use log::{info, warn};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;

pub(super) struct RemoteFile {
    pub name: &'static str,
    pub size: u64,
    /// `None` for small text files Hugging Face does not store in LFS.
    pub sha256: Option<&'static str>,
}

pub(super) struct RemoteSet {
    /// Base URL; each file is fetched from `{base}/{name}`. Pinned to a
    /// commit so a repo update can never mix files from two revisions.
    pub base: &'static str,
    pub files: &'static [RemoteFile],
}

impl RemoteSet {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

pub(super) const QWEN3_ASR_1_7B: RemoteSet = RemoteSet {
    base: "https://huggingface.co/OpenVoiceOS/qwen3-asr-1.7b-onnx/resolve/895b989354254bda979d2ba2fc413007ac89361c",
    files: &[
        RemoteFile {
            name: "config.json",
            size: 5_512,
            sha256: None,
        },
        RemoteFile {
            name: "vocab.json",
            size: 3_081_557,
            sha256: None,
        },
        RemoteFile {
            name: "encoder_int8.onnx",
            size: 318_943_605,
            sha256: Some("46370ace9867341b3263fc6d291846329c8a8a0ea791ad18c5843dfd3ac19d9b"),
        },
        RemoteFile {
            name: "embed_tokens_int8.onnx",
            size: 311_165_460,
            sha256: Some("6ba19fb12bb925ea06efa452746db84496368d3656f08930b964d8f0345b8db6"),
        },
        RemoteFile {
            name: "decoder_int8.onnx",
            size: 1_723_241_732,
            sha256: Some("c6f5d491fd5e2c1a604092afe17d5681148ff5b7e067afa71dbbc793bf496b82"),
        },
    ],
};

/// The loose-file source of a catalogue model, if it has one.
pub(super) fn remote_set(model_id: &str) -> Option<&'static RemoteSet> {
    match model_id {
        super::QWEN3_ASR_MODEL_ID => Some(&QWEN3_ASR_1_7B),
        _ => None,
    }
}

/// Bytes on disk under a partial download, whether it is a file or a directory.
pub(super) fn partial_size(path: &Path) -> u64 {
    if path.is_dir() {
        fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.metadata().ok())
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    } else {
        path.metadata().map(|m| m.len()).unwrap_or(0)
    }
}

enum FileOutcome {
    Done,
    Cancelled,
}

struct Progress<'a> {
    manager: &'a ModelManager,
    model_id: &'a str,
    done_before: u64,
    total: u64,
    last_emit: Instant,
}

impl Progress<'_> {
    fn emit(&mut self, file_bytes: u64, force: bool) {
        if !force && self.last_emit.elapsed() < Duration::from_millis(100) {
            return;
        }
        let downloaded = self.done_before + file_bytes;
        let _ = self.manager.app_handle.emit(
            "model-download-progress",
            &DownloadProgress {
                model_id: self.model_id.to_string(),
                downloaded,
                total: self.total,
                percentage: if self.total > 0 {
                    downloaded as f64 / self.total as f64 * 100.0
                } else {
                    0.0
                },
            },
        );
        self.last_emit = Instant::now();
    }
}

impl ModelManager {
    pub(super) async fn download_multi_file_model(
        &self,
        model_id: &str,
        set: &'static RemoteSet,
    ) -> Result<()> {
        let model_info = self
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;
        let final_dir = self.models_dir.join(&model_info.filename);
        let partial_dir = self
            .models_dir
            .join(format!("{}.partial", model_info.filename));

        let cancel_flag = Arc::new(AtomicBool::new(false));
        {
            let mut flags = self.cancel_flags.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = flags.get(model_id) {
                return Err(anyhow::anyhow!(if existing.load(Ordering::Relaxed) {
                    "The cancelled download of this model is still stopping; try again in a moment"
                } else {
                    "This model is already downloading"
                }));
            }
            flags.insert(model_id.to_string(), cancel_flag.clone());
        }
        let cleanup = DownloadCleanup {
            available_models: &self.available_models,
            cancel_flags: &self.cancel_flags,
            model_id: model_id.to_string(),
            flag: cancel_flag.clone(),
        };

        if final_dir.is_dir() {
            let _ = fs::remove_dir_all(&partial_dir);
            drop(cleanup);
            self.update_download_status()?;
            return Ok(());
        }

        if partial_dir.is_file() {
            // Left by an older build that expected an archive here.
            fs::remove_file(&partial_dir)?;
        }
        fs::create_dir_all(&partial_dir)?;
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = true;
            }
        }

        let client = reqwest::Client::new();
        let mut progress = Progress {
            manager: self,
            model_id,
            done_before: 0,
            total: set.total_size(),
            last_emit: Instant::now(),
        };
        progress.emit(partial_size(&partial_dir), true);

        for file in set.files {
            let path = partial_dir.join(file.name);
            let url = format!("{}/{}", set.base, file.name);
            match self
                .fetch_file(&client, &url, &path, file, &cancel_flag, &mut progress)
                .await?
            {
                FileOutcome::Done => {}
                FileOutcome::Cancelled => {
                    info!("Download cancelled for: {}", model_id);
                    return Ok(());
                }
            }
            progress.done_before += file.size;
        }
        progress.emit(0, true);

        if final_dir.exists() {
            fs::remove_dir_all(&final_dir)?;
        }
        fs::rename(&partial_dir, &final_dir)?;

        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = false;
                model.is_downloaded = true;
                model.partial_size = 0;
            }
        }
        drop(cleanup);
        let _ = self.app_handle.emit("model-download-complete", model_id);
        info!(
            "Successfully downloaded model {} to {:?}",
            model_id, final_dir
        );
        Ok(())
    }

    /// Fetch one file (resuming a partial copy) and verify size and checksum.
    /// A file that fails verification is deleted so a retry starts it fresh.
    async fn fetch_file(
        &self,
        client: &reqwest::Client,
        url: &str,
        path: &Path,
        file: &RemoteFile,
        cancel_flag: &AtomicBool,
        progress: &mut Progress<'_>,
    ) -> Result<FileOutcome> {
        let mut have = path.metadata().map(|m| m.len()).unwrap_or(0);
        if have > file.size {
            fs::remove_file(path)?;
            have = 0;
        }

        if have < file.size {
            let mut request = client.get(url);
            if have > 0 {
                request = request.header("Range", format!("bytes={}-", have));
            }
            let mut response = request.send().await?;
            let mut complete = false;
            if have > 0 {
                let content_range = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                match classify_resume(response.status().as_u16(), have, content_range.as_deref()) {
                    ResumeOutcome::Continue => {}
                    ResumeOutcome::AlreadyComplete => complete = true,
                    ResumeOutcome::Restart | ResumeOutcome::DiscardPartial => {
                        drop(response);
                        let _ = fs::remove_file(path);
                        have = 0;
                        response = client.get(url).send().await?;
                    }
                }
            }

            if !complete {
                if !response.status().is_success() {
                    return Err(anyhow::anyhow!(
                        "Failed to download {}: HTTP {}",
                        url,
                        response.status()
                    ));
                }
                let mut out = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                let mut stream = response.bytes_stream();
                let mut last_byte_at = Instant::now();
                loop {
                    if cancel_flag.load(Ordering::Relaxed) {
                        return Ok(FileOutcome::Cancelled);
                    }
                    let chunk =
                        match tokio::time::timeout(Duration::from_secs(1), stream.next()).await {
                            Ok(Some(chunk)) => chunk?,
                            Ok(None) => break,
                            Err(_) => {
                                if last_byte_at.elapsed() >= DOWNLOAD_STALL_TIMEOUT {
                                    return Err(anyhow::anyhow!(
                                        "Download stalled: no data for {}s",
                                        DOWNLOAD_STALL_TIMEOUT.as_secs()
                                    ));
                                }
                                continue;
                            }
                        };
                    last_byte_at = Instant::now();
                    out.write_all(&chunk)?;
                    have += chunk.len() as u64;
                    progress.emit(have, false);
                }
                out.flush()?;
            }
        }

        let actual = path.metadata()?.len();
        if actual != file.size {
            let _ = fs::remove_file(path);
            return Err(anyhow::anyhow!(
                "Download of {} incomplete: expected {} bytes, got {}",
                file.name,
                file.size,
                actual
            ));
        }

        if let Some(expected) = file.sha256 {
            let _ = self
                .app_handle
                .emit("model-verification-started", progress.model_id);
            let verify_path = path.to_path_buf();
            let label = format!("{}/{}", progress.model_id, file.name);
            let expected = expected.to_string();
            tokio::task::spawn_blocking(move || {
                ModelManager::verify_sha256(&verify_path, Some(&expected), &label)
            })
            .await
            .map_err(|e| anyhow::anyhow!("SHA256 task panicked: {}", e))??;
            let _ = self
                .app_handle
                .emit("model-verification-completed", progress.model_id);
        } else if file.name.ends_with(".json") {
            // Not in LFS, so no published hash: at least make sure an error
            // page did not land under the right name.
            let text = fs::read_to_string(path)?;
            if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                let _ = fs::remove_file(path);
                warn!("{} is not valid JSON; discarded", file.name);
                return Err(anyhow::anyhow!("Downloaded {} is corrupt", file.name));
            }
        }
        Ok(FileOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_size_sums_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("b"), [0u8; 5]).unwrap();
        assert_eq!(partial_size(dir.path()), 15);
        assert_eq!(partial_size(&dir.path().join("a")), 10);
        assert_eq!(partial_size(&dir.path().join("missing")), 0);
    }

    #[test]
    fn qwen_set_is_pinned_and_complete() {
        assert!(QWEN3_ASR_1_7B.base.contains("/resolve/895b9893"));
        let names: Vec<_> = QWEN3_ASR_1_7B.files.iter().map(|f| f.name).collect();
        for needed in [
            "config.json",
            "vocab.json",
            "encoder_int8.onnx",
            "embed_tokens_int8.onnx",
            "decoder_int8.onnx",
        ] {
            assert!(names.contains(&needed), "{needed} missing");
        }
        assert_eq!(QWEN3_ASR_1_7B.total_size() / 1_000_000, 2_356);
    }
}
