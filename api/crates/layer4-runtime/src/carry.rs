//! The envelope a conversation travels in.
//!
//! A conversation is carried between pods as opaque bytes, and that opacity is
//! deliberate: the agent's file is the agent's own, and anything that parsed it
//! would break the day the agent changed its shape. But bytes alone lost
//! something the sibling module needs — *where* the file was — and a store laid
//! out as a keyed tree cannot be rebuilt without it. A conversation put back in
//! the wrong place is not found, and the agent starts a fresh one and reports
//! success.
//!
//! So one line is added in front, written and read here and nowhere else. It
//! names the path the file was found at, relative to the directory the adapter
//! nominated. That is still not knowledge of a format: this reads back a line
//! it wrote itself, never the agent's content, which follows it verbatim.
//!
//! ## Two dialects, and why the older one keeps working
//!
//! A pod that predates this sends bare bytes, and a rollout is in flight
//! between pods of different vintages exactly when §5 matters most. So a
//! payload that is not an envelope is not an error: it is the older dialect,
//! and it gets the placement it has always had. A session is never lost by
//! being carried in the direction the fleet happens to be rolling.

use std::path::{Component, Path, PathBuf};

/// Marks the header line, and says which envelope this is.
///
/// Distinctive enough that no agent's own first line is mistaken for one: it is
/// a JSON object whose first key names this service.
const MARKER: &str = "chronoConversation";

/// The envelope version. Bumped only if the header's meaning changes.
const VERSION: u64 = 1;

/// Wrap a conversation with the path it was found at.
pub fn seal(relative: &Path, bytes: &[u8]) -> Vec<u8> {
    let header = serde_json::json!({
        MARKER: VERSION,
        "path": relative.to_string_lossy(),
    });
    let mut sealed = header.to_string().into_bytes();
    sealed.push(b'\n');
    sealed.extend_from_slice(bytes);
    sealed
}

/// Read a carried conversation: where it belongs, and what it is.
///
/// `None` for the path means the sender did not say — an older pod, or a
/// header this build cannot read. Either way the payload is handed back whole,
/// because the bytes are the part that must never be lost.
pub fn open(carried: &[u8]) -> (Option<PathBuf>, &[u8]) {
    let Some(end) = carried.iter().position(|b| *b == b'\n') else {
        return (None, carried);
    };
    let Ok(header) = serde_json::from_slice::<serde_json::Value>(&carried[..end]) else {
        return (None, carried);
    };
    let Some(version) = header.get(MARKER).and_then(serde_json::Value::as_u64) else {
        return (None, carried);
    };
    // Ours, so the line is stripped whatever it says — leaving it in would put
    // a line the agent never wrote at the top of its own file. A version this
    // build does not implement falls back on placement only.
    let payload = &carried[end + 1..];
    if version != VERSION {
        tracing::warn!(
            version,
            "a conversation arrived in an envelope this build cannot read"
        );
        return (None, payload);
    }
    let path = header
        .get("path")
        .and_then(serde_json::Value::as_str)
        .and_then(safe_relative);
    if path.is_none() {
        // The header was ours and its path was not usable. Said out loud
        // rather than passed over: the conversation still lands somewhere, but
        // a kernel that needed it in its own place will not find it.
        tracing::warn!("a carried conversation named a path that cannot be trusted");
    }
    (path, payload)
}

/// A relative path that cannot leave the directory it is joined to.
///
/// The bytes crossed an HTTP hop from a pod. A path taken on trust is a write
/// to anywhere this process can reach, so anything absolute, anything with a
/// parent segment, and anything with a root or prefix is refused outright
/// rather than sanitized — a path that needed cleaning is not one to guess at.
fn safe_relative(raw: &str) -> Option<PathBuf> {
    let path = Path::new(raw);
    if raw.is_empty() || path.is_absolute() {
        return None;
    }
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            // A bare `.` says nothing and is dropped; everything else either
            // escapes or names a root.
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
#[path = "carry_tests.rs"]
mod carry_tests;
