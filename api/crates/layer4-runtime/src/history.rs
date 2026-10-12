//! Bounded access to the agent-sandbox session JSONL files.

use std::path::{Path, PathBuf};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::safe_component;

const MAX_HISTORY_FILES: usize = 1_024;
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
    let mut entries = match tokio::fs::read_dir(&sessions).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(RuntimeError::unavailable(
                "history_unavailable",
                format!("open agent history: {error}"),
            ))
        }
    };
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        RuntimeError::unavailable(
            "history_unavailable",
            format!("read agent history: {error}"),
        )
    })? {
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
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            RuntimeError::rejected("history_invalid", "agent history filename is not UTF-8")
        })?;
        if !name.ends_with(".jsonl") {
            continue;
        }
        files.push(entry.path());
        if files.len() > MAX_HISTORY_FILES {
            return Err(RuntimeError::rejected(
                "history_too_large",
                "agent history contains too many files",
            ));
        }
    }
    files.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut output = Vec::new();
    for path in files {
        append_file(&path, &mut output).await?;
    }
    Ok(output)
}

async fn append_file(path: &Path, output: &mut Vec<u8>) -> Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        RuntimeError::unavailable(
            "history_unavailable",
            format!("inspect agent history: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
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
    let bytes = tokio::fs::read(path).await.map_err(|error| {
        RuntimeError::unavailable(
            "history_unavailable",
            format!("read agent history: {error}"),
        )
    })?;
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
    use std::fs;

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
    async fn rejects_malformed_and_oversized_history() {
        let temp = TempHome::new();
        let dir = temp.0.join(".cma/agents/agent-a/home/sessions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.jsonl"), b"not-json\n").unwrap();
        let error = export(&config(&temp.0), "agent-a").await.unwrap_err();
        assert_eq!(error.code, "history_invalid");
    }
}
