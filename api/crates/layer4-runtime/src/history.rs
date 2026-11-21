//! Bounded access to the agent-sandbox session JSONL files.

use std::path::{Path, PathBuf};

use tokio::io::AsyncReadExt as _;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::safe_component;

const MAX_HISTORY_FILES: usize = 1_024;
const MAX_HISTORY_ENTRIES: usize = 16 * 1_024;
const MAX_HISTORY_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;

/// Read one agent's durable session files in deterministic order.
///
/// The runtime owns these files.  The route validates every emitted line as
/// JSON before returning it, refuses symlinks, and applies both per-file and
/// aggregate bounds so a corrupted home cannot turn a download into an
/// unbounded read.
pub(crate) async fn export(config: &RuntimeConfig, agent_id: &str) -> Result<Vec<u8>> {
    safe_component(agent_id, "agent id")?;
    let sessions = config.agent_home(agent_id).join("sessions");
    let metadata = match tokio::fs::symlink_metadata(&sessions).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(RuntimeError::unavailable(
                "history_unavailable",
                format!("inspect agent history: {error}"),
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeError::rejected(
            "history_invalid",
            "agent history sessions path is not a directory",
        ));
    }
    let mut directories = vec![sessions];
    let mut files = Vec::<PathBuf>::new();
    let mut entries_seen = 0_usize;
    while let Some(directory) = directories.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await.map_err(|error| {
            RuntimeError::unavailable(
                "history_unavailable",
                format!("open agent history directory: {error}"),
            )
        })?;
        while let Some(entry) = entries.next_entry().await.map_err(|error| {
            RuntimeError::unavailable(
                "history_unavailable",
                format!("read agent history: {error}"),
            )
        })? {
            entries_seen = entries_seen.saturating_add(1);
            if entries_seen > MAX_HISTORY_ENTRIES {
                return Err(RuntimeError::rejected(
                    "history_too_large",
                    "agent history contains too many entries",
                ));
            }
            let file_type = entry.file_type().await.map_err(|error| {
                RuntimeError::unavailable(
                    "history_unavailable",
                    format!("inspect agent history entry: {error}"),
                )
            })?;
            if file_type.is_symlink() {
                return Err(RuntimeError::rejected(
                    "history_invalid",
                    "agent history contains a symlink",
                ));
            }
            if file_type.is_dir() {
                directories.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                RuntimeError::rejected("history_invalid", "agent history filename is not UTF-8")
            })?;
            if name.ends_with(".jsonl") {
                files.push(entry.path());
                if files.len() > MAX_HISTORY_FILES {
                    return Err(RuntimeError::rejected(
                        "history_too_large",
                        "agent history contains too many files",
                    ));
                }
            }
        }
    }
    files.sort();

    let mut output = Vec::new();
    for path in files {
        append_file(&path, &mut output).await?;
    }
    Ok(output)
}

async fn append_file(path: &Path, output: &mut Vec<u8>) -> Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).await.map_err(|error| {
        RuntimeError::unavailable(
            "history_unavailable",
            format!("read agent history: {error}"),
        )
    })?;
    let metadata = file.metadata().await.map_err(|error| {
        RuntimeError::unavailable(
            "history_unavailable",
            format!("inspect agent history: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(RuntimeError::rejected(
            "history_invalid",
            "agent history entry changed while exporting",
        ));
    }
    if metadata.len() > MAX_HISTORY_FILE_BYTES {
        return Err(RuntimeError::rejected(
            "history_too_large",
            "agent history file exceeds its size limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_HISTORY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            RuntimeError::unavailable(
                "history_unavailable",
                format!("read agent history: {error}"),
            )
        })?;
    if bytes.len() as u64 > MAX_HISTORY_FILE_BYTES {
        return Err(RuntimeError::rejected(
            "history_too_large",
            "agent history file exceeds its size limit",
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        RuntimeError::rejected("history_invalid", "agent history is not UTF-8 JSONL")
    })?;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line).map_err(|_| {
            RuntimeError::rejected("history_invalid", "agent history contains malformed JSON")
        })?;
        let next = output.len().checked_add(line.len() + 1).ok_or_else(|| {
            RuntimeError::rejected("history_too_large", "agent history exceeds its size limit")
        })?;
        if next > MAX_HISTORY_BYTES {
            return Err(RuntimeError::rejected(
                "history_too_large",
                "agent history exceeds its size limit",
            ));
        }
        output.extend_from_slice(line.as_bytes());
        output.push(b'\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    struct TempHome(PathBuf);

    impl TempHome {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("cma-history-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn config(home: &Path) -> RuntimeConfig {
        RuntimeConfig::for_test(home.to_path_buf())
    }

    #[tokio::test]
    async fn exports_sorted_jsonl_and_ignores_non_session_files() {
        let temp = TempHome::new();
        let dir = temp.0.join(".cma/agents/agent-a/home/sessions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("z.jsonl"), b"{\"n\":2}\n").unwrap();
        fs::write(dir.join("a.jsonl"), b"{\"n\":1}").unwrap();
        fs::write(dir.join("notes.txt"), b"not json").unwrap();
        let bytes = export(&config(&temp.0), "agent-a").await.unwrap();
        assert_eq!(bytes, b"{\"n\":1}\n{\"n\":2}\n");
    }

    #[tokio::test]
    async fn exports_nested_codex_session_files_in_path_order() {
        let temp = TempHome::new();
        let root = temp.0.join(".cma/agents/agent-a/home/sessions");
        let nested = root.join("2026/10/13");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("2026.jsonl"), b"{\"n\":1}\n").unwrap();
        fs::write(nested.join("rollout.jsonl"), b"{\"n\":2}\n").unwrap();
        let bytes = export(&config(&temp.0), "agent-a").await.unwrap();
        assert_eq!(bytes, b"{\"n\":2}\n{\"n\":1}\n");
    }

    #[tokio::test]
    async fn rejects_malformed_and_oversized_history() {
        let temp = TempHome::new();
        let dir = temp.0.join(".cma/agents/agent-a/home/sessions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.jsonl"), b"not-json\n").unwrap();
        let error = export(&config(&temp.0), "agent-a").await.unwrap_err();
        assert_eq!(error.code, "history_invalid");
    }
}
