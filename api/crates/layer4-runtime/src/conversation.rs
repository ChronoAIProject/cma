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
//! Naive about the *layout* too, and that is newer. Where the agent keeps its
//! rollouts used to be a constant here — one kernel's directory name, in the
//! supervisor that is supposed to host any of them. It is now told to this pod
//! by the adapter that started it, because the layer that knows a kernel's name
//! is the layer that owns that kernel.
//!
//! Only the newest rollout is carried. A pod hosts one session, so any older
//! ones belong to conversations that already ended, and sending them would
//! grow every subsequent handover without adding anything resumable.

use std::io::Write;
use std::path::{Path, PathBuf};

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
pub fn capture(home: &Path, sessions_dir: &str) -> std::io::Result<Option<Vec<u8>>> {
    let Some(newest) = newest_rollout(&home.join(sessions_dir))? else {
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

/// A conversation laid down for the agent to continue.
pub struct Restored {
    /// Where the rollout was written. Read by the tests that check it landed
    /// somewhere the agent looks.
    #[cfg_attr(not(test), allow(dead_code))]
    pub path: PathBuf,
    /// The conversation's own identifier, taken from the rollout.
    ///
    /// The agent has to be told which conversation to continue by name. It
    /// will not work it out: asked for "the most recent", it selects among
    /// conversations recorded in the current working directory, and a
    /// conversation carried from another pod was recorded somewhere else. It
    /// then finds nothing, starts a fresh one, and reports success — so a
    /// missing identifier here is silent data loss, not a failure.
    pub session_id: Option<String>,
}

/// Lay a captured conversation down where the agent will find it.
pub fn restore(
    home: &Path,
    sessions_dir: &str,
    bytes: &[u8],
    stamp: &str,
) -> std::io::Result<Restored> {
    let dir = home.join(sessions_dir).join("restored");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("rollout-{stamp}.jsonl"));
    let mut file = std::fs::File::create(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let session_id = conversation_id(bytes);
    match session_id.as_deref() {
        Some(id) => tracing::info!(
            path = %path.display(),
            bytes = bytes.len(),
            conversation = %id,
            "restored the conversation"
        ),
        // Worth shouting about: without the identifier the agent cannot be
        // told what to continue, and will quietly start something new.
        None => tracing::error!(
            path = %path.display(),
            bytes = bytes.len(),
            "restored a conversation that does not name itself; the agent cannot be told to continue it"
        ),
    }
    Ok(Restored { path, session_id })
}

/// The identifier a rollout gives itself, from its opening record.
///
/// Read rather than derived from the filename: the name is ours to choose and
/// the agent's own record is not, so this keeps working when we change how the
/// file is named and when the agent changes how it names them.
///
/// `None` is an ordinary answer, not a failure. This shape — a first line
/// whose `payload.id` names the conversation — belongs to one kernel's
/// rollout format, and a kernel that records something else, or something that
/// is not text at all, simply has no identifier to give. Whether that matters
/// is decided by whoever asked, not here: it matters only to a kernel that has
/// to be told by name which conversation to continue.
fn conversation_id(bytes: &[u8]) -> Option<String> {
    let first = bytes.split(|b| *b == b'\n').next()?;
    let value: serde_json::Value = serde_json::from_slice(first).ok()?;
    value
        .get("payload")?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// The most recently modified rollout under a directory tree.
fn newest_rollout(root: &Path) -> std::io::Result<Option<PathBuf>> {
    if !root.exists() {
        return Ok(None);
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut pending = vec![root.to_path_buf()];

    // Every regular file is a candidate, whatever it is called. One kernel
    // writes newline-delimited JSON and another writes compressed frames; a
    // filter on the extension would be this module knowing a format again,
    // which is the thing it exists not to know.
    //
    // Iterative rather than recursive: rollouts are filed under a directory
    // tree from disk, and that is not something to recurse into without a
    // bound on depth.
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            if !kind.is_file() {
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

    /// A stand-in for whatever an adapter tells this pod its kernel uses.
    const SESSIONS_DIR: &str = ".a-kernel/sessions";

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
        assert!(capture(dir.path(), SESSIONS_DIR)
            .expect("readable")
            .is_none());
    }

    #[test]
    fn the_conversation_is_carried_verbatim() {
        // Byte-exact on purpose. Anything that rewrites the agent's own file
        // is something that breaks when the agent changes its format.
        let dir = Home::new("verbatim");
        let body = "{\"turn\":1}\n{\"turn\":2}\n";
        write_rollout(dir.path(), "2026/08/14", "rollout-a.jsonl", body);

        let captured = capture(dir.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present");
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

        let captured = capture(dir.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present");
        assert_eq!(captured, b"new");
    }

    #[test]
    fn what_is_carried_does_not_depend_on_what_a_file_is_called() {
        // The supervisor hosts any kernel, and kernels do not agree on how a
        // conversation is stored — one writes newline-delimited JSON, another
        // compressed frames. Filtering on a filename would be this module
        // knowing a format, which is the thing it exists not to know.
        //
        // The safer failure runs this way round. Carrying something the next
        // agent cannot read is caught downstream and refuses the rotation,
        // leaving the session where it is; carrying nothing because the name
        // was unfamiliar reads as "no conversation" and loses the history for
        // real.
        let dir = Home::new("any-filename");
        write_rollout(dir.path(), "2026/08/14", "store.zst", "carried anyway");
        assert_eq!(
            capture(dir.path(), SESSIONS_DIR)
                .expect("readable")
                .expect("present"),
            b"carried anyway"
        );
    }

    #[test]
    fn a_conversation_that_names_itself_in_another_shape_still_restores() {
        // The first-line `payload.id` belongs to one kernel's rollout format.
        // A kernel that records something else — or something that is not text
        // at all — has no identifier to give, and that is an ordinary answer.
        // Failing here instead would make this module's one piece of format
        // knowledge into a requirement every kernel has to satisfy.
        let dir = Home::new("foreign-shape");
        let restored = restore(
            dir.path(),
            SESSIONS_DIR,
            &[0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x01],
            "2026-08-17T00-00-00",
        )
        .expect("a conversation in an unfamiliar shape is still laid down");
        assert!(restored.path.exists());
        assert!(
            restored.session_id.is_none(),
            "there is no identifier to invent"
        );
    }

    /// A rollout's opening record, in the shape the agent writes it.
    const META: &str = r#"{"timestamp":"2026-08-14T00:00:00Z","type":"session_meta","payload":{"id":"019ed5ef-b5eb-7f60-96d1-8b8202fa7eff","cwd":"/somewhere/else"}}"#;

    #[test]
    fn a_restored_conversation_names_itself() {
        // The identifier is the whole mechanism: the agent continues a
        // conversation by name, and asked for "the most recent" it would
        // select among conversations recorded in this working directory —
        // which a conversation carried from another pod never is.
        let dir = Home::new("names-itself");
        let restored = restore(
            dir.path(),
            SESSIONS_DIR,
            format!("{META}\n").as_bytes(),
            "stamp",
        )
        .expect("restorable");
        assert_eq!(
            restored.session_id.as_deref(),
            Some("019ed5ef-b5eb-7f60-96d1-8b8202fa7eff")
        );
    }

    #[test]
    fn a_conversation_that_does_not_name_itself_is_not_guessed_at() {
        // Better to start fresh loudly than to resume something arbitrary.
        let dir = Home::new("unnamed");
        let restored =
            restore(dir.path(), SESSIONS_DIR, b"not json at all\n", "stamp").expect("restorable");
        assert!(restored.session_id.is_none());
    }

    #[test]
    fn a_restored_conversation_is_what_the_agent_finds() {
        // The round trip is the whole point: what one pod captured is what the
        // next one resumes from.
        let source = Home::new("round-trip-source");
        let body = "{\"turn\":1}\n";
        write_rollout(source.path(), "2026/08/14", "rollout-a.jsonl", body);
        let captured = capture(source.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present");

        let target = Home::new("round-trip-target");
        restore(
            target.path(),
            SESSIONS_DIR,
            &captured,
            "2026-08-14T00-00-00",
        )
        .expect("restorable");

        let found = capture(target.path(), SESSIONS_DIR)
            .expect("readable")
            .expect("present");
        assert_eq!(found, body.as_bytes());
    }

    #[test]
    fn restoring_creates_the_directory_the_agent_reads() {
        // A fresh pod has no sessions tree at all, so restore cannot assume one.
        let dir = Home::new("round-trip");
        let restored =
            restore(dir.path(), SESSIONS_DIR, b"{}", "2026-08-14T00-00-00").expect("restorable");
        assert!(restored.path.exists());
        assert!(restored.path.starts_with(dir.path().join(SESSIONS_DIR)));
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

        assert!(capture(dir.path(), SESSIONS_DIR).is_err());
    }
}
