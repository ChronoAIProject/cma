use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

use layer3_adapter::ConversationSnapshot;
use sha2::{Digest, Sha256};

use crate::command::hex;
use crate::failure::{Result, RuntimeError};

const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OPENING_BYTES: u64 = 64 * 1024;
const MAX_ENTRIES: usize = 100_000;

pub(crate) fn capture(agent_home: &Path) -> Result<ConversationSnapshot> {
    let sessions = agent_home.join("sessions");
    let path = newest_file(&sessions)?.ok_or_else(|| {
        RuntimeError::rejected(
            "conversation_not_found",
            "agent home contains no conversation record",
        )
    })?;
    let metadata = std::fs::metadata(&path).map_err(capture_io("inspect conversation"))?;
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(RuntimeError::rejected(
            "conversation_too_large",
            format!("conversation exceeds {MAX_ARCHIVE_BYTES} bytes"),
        ));
    }
    let mut archive = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(&path)
        .map_err(capture_io("open conversation"))?
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut archive)
        .map_err(capture_io("read conversation"))?;
    if archive.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(RuntimeError::rejected(
            "conversation_too_large",
            format!("conversation exceeds {MAX_ARCHIVE_BYTES} bytes"),
        ));
    }
    let conversation_id = conversation_id(&archive).ok_or_else(|| {
        RuntimeError::rejected(
            "conversation_id_missing",
            "newest conversation record does not name its conversation",
        )
    })?;
    Ok(ConversationSnapshot {
        conversation_id,
        archive_sha256: hex(&Sha256::digest(&archive)),
        archive,
    })
}

fn newest_file(root: &Path) -> Result<Option<PathBuf>> {
    if !root.exists() {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(root).map_err(capture_io("inspect sessions"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::rejected(
            "sessions_invalid",
            "agent sessions path is not a directory",
        ));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0_usize;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(capture_io("list sessions"))? {
            let entry = entry.map_err(capture_io("read sessions entry"))?;
            visited += 1;
            if visited > MAX_ENTRIES {
                return Err(RuntimeError::rejected(
                    "sessions_too_large",
                    "agent sessions tree contains too many entries",
                ));
            }
            let kind = entry
                .file_type()
                .map_err(capture_io("inspect sessions entry"))?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .map_err(capture_io("inspect conversation timestamp"))?;
                let path = entry.path();
                if newest
                    .as_ref()
                    .is_none_or(|(time, prior)| (modified, &path) > (*time, prior))
                {
                    newest = Some((modified, path));
                }
            }
        }
    }
    Ok(newest.map(|(_, path)| path))
}

fn conversation_id(bytes: &[u8]) -> Option<String> {
    let mut first = Vec::new();
    std::io::BufReader::new(bytes)
        .take(MAX_OPENING_BYTES + 1)
        .read_until(b'\n', &mut first)
        .ok()?;
    if first.len() as u64 > MAX_OPENING_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&first).ok()?;
    value
        .get("payload")?
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(str::to_string)
}

fn capture_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| {
        RuntimeError::unavailable("conversation_io_failed", format!("{operation}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_rollout_is_captured_with_its_own_id_and_digest() {
        let home = std::env::temp_dir().join(format!("cma-conversation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("sessions/2026/09")).unwrap();
        let bytes = br#"{"payload":{"id":"conversation-7"}}
{"event":"answer"}
"#;
        std::fs::write(home.join("sessions/2026/09/rollout.jsonl"), bytes).unwrap();
        let snapshot = capture(&home).unwrap();
        assert_eq!(snapshot.conversation_id, "conversation-7");
        assert_eq!(snapshot.archive, bytes);
        assert_eq!(snapshot.archive_sha256, hex(&Sha256::digest(bytes)));
        std::fs::remove_dir_all(home).unwrap();
    }
}
