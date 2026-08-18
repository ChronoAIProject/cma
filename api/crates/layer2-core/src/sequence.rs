//! Where a session's next event sequence comes from.
//!
//! A session's log is ordered by a per-session sequence, and that number is
//! what a client resumes on. It has to be allocated by exactly one thing.
//!
//! It was allocated by two. A live session's events were numbered off the
//! in-memory entry, and provisioning, revival, retirement and rotation each
//! numbered theirs off a record read from the store — two counters for one
//! session, advancing independently. Under contention they agreed on a number,
//! the unique index refused the second batch, and a turn that had already run
//! left nothing in the log for any interface to report. That is not a
//! rendering bug or a lost log line: the durable log is what a client is told
//! to recover from, so an event never written is unrecoverable by any route.
//!
//! So: one allocator per process, and every producer goes through it.
//!
//! **It is deliberately not the whole guarantee.** The allocator settles
//! contention inside one control plane; the control plane is horizontally
//! scalable, and a restarted process seeds from a record that a concurrent
//! whole-document write may have left behind the log. What closes those is the
//! store refusing a taken sequence and the caller re-numbering — the database
//! is the authority, and this is only the thing that stops us racing ourselves.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::ids::SessionId;

/// The one place a session's next event sequence is decided.
///
/// Synchronous on purpose. Numbering happens while the entries lock is held,
/// and an allocator that had to be awaited would either force that lock open
/// or push the numbering somewhere it could race again. The lock here is held
/// across a map lookup and an addition, and never across an await.
#[derive(Default)]
pub(crate) struct Sequences {
    /// The highest sequence handed out for a session, while this process runs.
    issued: Mutex<HashMap<SessionId, u64>>,
}

impl Sequences {
    /// Hand out `count` consecutive sequences and return the first.
    ///
    /// `floor` is the highest sequence the caller knows to be used — from the
    /// entry it holds, or from the record it just read. It is passed in rather
    /// than looked up because this cannot do I/O, and because the durable
    /// record is what a freshly started process has to start from. Whichever
    /// is higher wins, so a caller holding a stale copy cannot pull the
    /// sequence backwards over one that has already been issued.
    pub(crate) fn take(&self, id: &SessionId, floor: u64, count: u64) -> u64 {
        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        let last = issued.entry(id.clone()).or_insert(0);
        *last = (*last).max(floor);
        let first = *last + 1;
        *last += count;
        first
    }

    /// The highest sequence handed out, never below `floor`.
    ///
    /// What a writer records as the session's position. Written from here
    /// rather than from whatever copy the writer holds, because the copy is
    /// only ever as current as the last time it was read, and storing a
    /// position behind the log is what makes the next process start too low.
    pub(crate) fn issued(&self, id: &SessionId, floor: u64) -> u64 {
        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        let last = issued.entry(id.clone()).or_insert(0);
        *last = (*last).max(floor);
        *last
    }

    /// Drop what is remembered about a session.
    ///
    /// Only for a session whose events are being deleted with it. A session
    /// that merely stopped keeps its position: it can be revived, and its log
    /// continues from where it left off rather than starting again.
    pub(crate) fn forget(&self, id: &SessionId) {
        self.issued
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
}

#[cfg(test)]
#[path = "sequence_tests.rs"]
mod sequence_tests;
