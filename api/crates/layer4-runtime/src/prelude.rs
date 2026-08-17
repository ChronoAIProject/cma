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
