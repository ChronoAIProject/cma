//! Capturing and restoring the agent's conversation.
//!
//! What makes a bot pod disposable. The agent records every turn to a rollout
//! file as it goes; carrying that file to a new pod and pointing the agent at
//! it is the difference between replacing a runtime and ending a session.
//!
//! Deliberately naive about the format. The rollout is the agent's own file in
//! the agent's own shape, and this reads and writes it verbatim — nothing here
//! parses a turn, which is what keeps this working across agent versions that
//! change what a turn looks like.
//!
//! Only the newest rollout is carried. A pod hosts one session, so any older
//! ones belong to conversations that already ended, and sending them would
//! grow every subsequent handover without adding anything resumable.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the agent keeps its rollouts, relative to home.
const SESSIONS_DIR: &str = ".codex/sessions";

/// Refuse anything implausible as a rollout.
///
/// A cap rather than a guess at what is reasonable: the bytes cross an HTTP
/// hop and land on a pod's disk, and an unbounded read is a way to run either
/// of those out. Generous enough that no real conversation reaches it.
const MAX_ROLLOUT_BYTES: u64 = 64 * 1024 * 1024;

/// The agent's most recent conversation, if it has had one.
///
/// `None` means the agent has not recorded anything yet, which is ordinary for
/// a session nobody has used and is not a failure.
pub fn capture(home: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let Some(newest) = newest_rollout(&home.join(SESSIONS_DIR))? else {
        return Ok(None);
    };
    let size = std::fs::metadata(&newest)?.len();
    if size > MAX_ROLLOUT_BYTES {
        return Err(std::io::Error::other(format!(
            "conversation is {size} bytes, beyond the {MAX_ROLLOUT_BYTES} that can be carried"
        )));
    }
    let bytes = std::fs::read(&newest)?;
    tracing::info!(path = %newest.display(), bytes = bytes.len(), "captured the conversation");
    Ok(Some(bytes))
}

/// Lay a captured conversation down where the agent will find it.
///
/// Written under today's date rather than the original's, because the agent
/// picks the most recent rollout and a file filed under its original date
/// would be passed over in favour of anything newer.
pub fn restore(home: &Path, bytes: &[u8], stamp: &str) -> std::io::Result<PathBuf> {
    let dir = home.join(SESSIONS_DIR).join("restored");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("rollout-{stamp}.jsonl"));
    let mut file = std::fs::File::create(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    tracing::info!(path = %path.display(), bytes = bytes.len(), "restored the conversation");
    Ok(path)
}

/// The most recently modified rollout under a directory tree.
fn newest_rollout(root: &Path) -> std::io::Result<Option<PathBuf>> {
    if !root.exists() {
        return Ok(None);
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut pending = vec![root.to_path_buf()];

    // Iterative rather than recursive: the agent files rollouts under
    // year/month/day, and a directory tree from disk is not something to
    // recurse into without a bound on depth.
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            if !kind.is_file() || path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let modified = entry.metadata()?.modified()?;
            if best.as_ref().is_none_or(|(when, _)| modified > *when) {
                best = Some((modified, path));
            }
        }
    }
    Ok(best.map(|(_, path)| path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home directory of this test's own, matching how the sibling module
    /// does it rather than taking a dependency to say the same thing.
    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("chrono-conversation-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a temporary home");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_rollout(home: &Path, day: &str, name: &str, body: &str) -> PathBuf {
        let dir = home.join(SESSIONS_DIR).join(day);
        std::fs::create_dir_all(&dir).expect("directories");
        let path = dir.join(name);
        std::fs::write(&path, body).expect("rollout");
        path
    }

    #[test]
    fn a_session_that_never_ran_has_nothing_to_carry() {
        // Not a failure: a pod may be replaced before anyone has used it, and
        // treating that as an error would refuse to move a healthy session.
        let dir = Home::new("never-ran");
        assert!(capture(dir.path()).expect("readable").is_none());
    }

    #[test]
    fn the_conversation_is_carried_verbatim() {
        // Byte-exact on purpose. Anything that rewrites the agent's own file
        // is something that breaks when the agent changes its format.
        let dir = Home::new("verbatim");
        let body = "{\"turn\":1}\n{\"turn\":2}\n";
        write_rollout(dir.path(), "2026/08/14", "rollout-a.jsonl", body);

        let captured = capture(dir.path()).expect("readable").expect("present");
        assert_eq!(captured, body.as_bytes());
    }

    #[test]
    fn the_newest_conversation_is_the_one_carried() {
        // A pod hosts one session, so older rollouts are finished
        // conversations. Carrying one of those would resume the wrong thing.
        let dir = Home::new("newest");
        write_rollout(dir.path(), "2026/08/13", "rollout-old.jsonl", "old");
        let newer = write_rollout(dir.path(), "2026/08/14", "rollout-new.jsonl", "new");
        // Modification times decide, and a freshly written file may share a
        // timestamp with its sibling, so this is made explicit.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&newer)
            .expect("openable")
            .set_modified(later)
            .expect("timestamps are settable");

        let captured = capture(dir.path()).expect("readable").expect("present");
        assert_eq!(captured, b"new");
    }

    #[test]
    fn files_that_are_not_rollouts_are_ignored() {
        // The sessions tree carries other things; resuming from one of them
        // would hand the agent something it cannot read.
        let dir = Home::new("not-rollouts");
        write_rollout(dir.path(), "2026/08/14", "notes.txt", "not a rollout");
        assert!(capture(dir.path()).expect("readable").is_none());
    }

    #[test]
    fn a_restored_conversation_is_what_the_agent_finds() {
        // The round trip is the whole point: what one pod captured is what the
        // next one resumes from.
        let source = Home::new("round-trip-source");
        let body = "{\"turn\":1}\n";
        write_rollout(source.path(), "2026/08/14", "rollout-a.jsonl", body);
        let captured = capture(source.path()).expect("readable").expect("present");

        let target = Home::new("round-trip-target");
        restore(target.path(), &captured, "2026-08-14T00-00-00").expect("restorable");

        let found = capture(target.path()).expect("readable").expect("present");
        assert_eq!(found, body.as_bytes());
    }

    #[test]
    fn restoring_creates_the_directory_the_agent_reads() {
        // A fresh pod has no sessions tree at all, so restore cannot assume one.
        let dir = Home::new("round-trip");
        let path = restore(dir.path(), b"{}", "2026-08-14T00-00-00").expect("restorable");
        assert!(path.exists());
        assert!(path.starts_with(dir.path().join(SESSIONS_DIR)));
    }

    #[test]
    fn an_implausibly_large_conversation_is_refused() {
        // The bytes cross an HTTP hop and land on a pod's disk. Reading
        // something unbounded is a way to exhaust either.
        let dir = Home::new("creates-dir");
        let path = write_rollout(dir.path(), "2026/08/14", "rollout-a.jsonl", "x");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("openable");
        file.set_len(MAX_ROLLOUT_BYTES + 1).expect("resizable");

        assert!(capture(dir.path()).is_err());
    }
}
