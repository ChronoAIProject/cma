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
//!
//! What *is* known, and now travels with the bytes, is where the file was
//! found. Not what it says — where it was. A store laid out as a keyed tree
//! cannot be rebuilt without it, and a conversation put back in the wrong place
//! is not found at all: the agent starts a fresh one and reports success. The
//! envelope that carries it lives in `carry`.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::carry;

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
    let root = home.join(sessions_dir);
    let Some(newest) = newest_rollout(&root)? else {
        return Ok(None);
    };
    let size = std::fs::metadata(&newest)?.len();
    if size > MAX_ROLLOUT_BYTES {
        return Err(std::io::Error::other(format!(
            "conversation is {size} bytes, beyond the {MAX_ROLLOUT_BYTES} that can be carried"
        )));
    }
    let bytes = std::fs::read(&newest)?;
    // Relative to the directory the adapter nominated, because that is the
    // only part a replacement pod can reproduce: its home is its own, and the
    // directory is what it was told.
    let relative = newest.strip_prefix(&root).unwrap_or(&newest);
    tracing::info!(
        path = %newest.display(),
        bytes = bytes.len(),
        "captured the conversation"
    );
    Ok(Some(carry::seal(relative, &bytes)))
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
    carried: &[u8],
    stamp: &str,
) -> std::io::Result<Restored> {
    let root = home.join(sessions_dir);
    let (relative, bytes) = carry::open(carried);
    // Where the sender found it, when it said. Otherwise the placement this
    // has always used, which is what a pod that predates the envelope gets and
    // what the first kernel's store accepts either way.
    let path = match relative {
        Some(relative) => root.join(relative),
        None => root.join("restored").join(format!("rollout-{stamp}.jsonl")),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
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
#[path = "conversation_tests.rs"]
mod conversation_tests;
