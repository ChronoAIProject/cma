//! Files a kernel needs on disk before it starts.
//!
//! The supervisor hosts any kernel and knows none, so it does not decide what
//! a kernel needs written or where — it is told both, writes what it is
//! handed, and reads neither. That is the same naivety the conversation carry
//! commits to, for the same reason: a kernel that changes its own format must
//! not be able to break the pod hosting it.
//!
//! This existed before as one kernel's configuration file, rendered here in
//! that kernel's own vocabulary at that kernel's own path. It worked while
//! there was one kernel. With three it wrote a file two of them do not read,
//! naming a provider route they do not use, into every one of their pods.
//!
//! A kernel that needs nothing written says nothing and nothing is written.

use std::path::{Component, Path, PathBuf};

/// Where the file goes, relative to the pod's home, and what goes in it.
///
/// Two variables rather than one structured value because the adapter that
/// sets them is composing an environment, and a path and a body are the two
/// things it has to say. Absent means this kernel needs no file.
pub const PATH_VAR: &str = "CHRONO_RUNTIME_CONFIG_PATH";

/// The file's content, verbatim.
pub const BODY_VAR: &str = "CHRONO_RUNTIME_CONFIG";

/// Write a kernel's configuration, if it was given one.
///
/// Returns where it was written, or `None` when nothing was asked for. A
/// failure is an error rather than a warning: a kernel started without the
/// configuration it was meant to have is a session that runs differently from
/// the one that was ordered, and silently.
pub fn write(home: &Path) -> std::io::Result<Option<PathBuf>> {
    let (Ok(relative), Ok(body)) = (std::env::var(PATH_VAR), std::env::var(BODY_VAR)) else {
        return Ok(None);
    };
    if relative.trim().is_empty() {
        return Ok(None);
    }
    let Some(relative) = contained(&relative) else {
        return Err(std::io::Error::other(format!(
            "{PATH_VAR} must be a path inside the pod's home, and {relative:?} is not"
        )));
    };
    let path = home.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, body.as_bytes())?;
    // Never with its content: what a kernel is configured with is between the
    // adapter that composed it and the agent that reads it, and it names an
    // endpoint this pod is proxying for somebody.
    tracing::info!(path = %path.display(), bytes = body.len(), "kernel configuration written");
    Ok(Some(path))
}

/// Marker naming this framing, and the version of it.
const PACK_MARKER: &str = "chronoPrelude";
const PACK_VERSION: u32 = 1;

/// The largest pack this supervisor will take.
///
/// A bound rather than a guess: the two skills a session carries by default
/// are about 400 KB together, and the cap is an order above that so a bigger
/// selection still lands while a runaway body is refused before it is read
/// into memory.
pub const PACK_LIMIT: usize = 4 * 1024 * 1024;

/// Files a kernel needs, carried in one body.
///
/// A JSON header line naming each path and its length, then the bodies
/// concatenated in that order. Not a tar: every entry needs the same
/// relative-path check either way, and a tar brings symlinks, hardlinks,
/// device nodes and modes that all have to be refused — a dependency to
/// express a header this module already knows how to write.
///
/// The paths are composed by whoever sends this, because where a kernel keeps
/// its files is that kernel's business and this process hosts any of them.
pub fn unpack(body: &[u8]) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    if body.len() > PACK_LIMIT {
        return Err(format!("a prelude may be at most {PACK_LIMIT} bytes"));
    }
    let split = body
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| "a prelude begins with a header line".to_string())?;
    let header: serde_json::Value = serde_json::from_slice(&body[..split])
        .map_err(|e| format!("the prelude header is not usable: {e}"))?;
    if header.get(PACK_MARKER).and_then(serde_json::Value::as_u64) != Some(u64::from(PACK_VERSION))
    {
        return Err("this is not a prelude this supervisor understands".to_string());
    }
    let entries = header
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the prelude header names no files".to_string())?;

    let mut at = split + 1;
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let raw = entry
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "a prelude entry has no path".to_string())?;
        let len = usize::try_from(
            entry
                .get("len")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("{raw} declares no length"))?,
        )
        .map_err(|_| format!("{raw} declares a length this platform cannot hold"))?;
        // Checked before reading, so a declared length longer than the body
        // is refused rather than panicking on the slice.
        let end = at
            .checked_add(len)
            .filter(|end| *end <= body.len())
            .ok_or_else(|| format!("{raw} declares more bytes than the prelude carries"))?;
        let path =
            contained(raw).ok_or_else(|| format!("{raw} is not a path inside the pod's home"))?;
        files.push((path, body[at..end].to_vec()));
        at = end;
    }
    Ok(files)
}

/// Write files a kernel needs, and say how many landed.
///
/// All or nothing is deliberately **not** attempted. A partial write is
/// visible — the kernel finds some of what it was promised — and unwinding
/// would mean deleting files this process did not create. The caller is told
/// which entry failed and decides.
pub fn write_all(home: &Path, files: &[(PathBuf, Vec<u8>)]) -> std::io::Result<usize> {
    for (relative, body) in files {
        let path = home.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, body)?;
    }
    // Never the contents: what a kernel is given is between the adapter that
    // composed it and the agent that reads it.
    tracing::info!(files = files.len(), "prelude files written");
    Ok(files.len())
}

/// A relative path that cannot leave the directory it is joined to.
///
/// The value arrives as environment set by a control plane, which is trusted —
/// but a path is the one kind of value where trusting is cheap to avoid, and
/// where being wrong writes a file somewhere this process should not.
fn contained(raw: &str) -> Option<PathBuf> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return None;
    }
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    match cleaned.as_os_str().is_empty() {
        true => None,
        false => Some(cleaned),
    }
}

#[cfg(test)]
#[path = "prelude_tests.rs"]
mod prelude_tests;
