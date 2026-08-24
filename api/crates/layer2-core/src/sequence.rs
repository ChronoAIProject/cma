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
use crate::store::StoreError;

/// The one place a session's next event sequence is decided.
///
/// Synchronous on purpose. The durable lane holds its own session lock across
/// numbering and FIFO insertion, so an allocator that had to be awaited would
/// split the exact boundary that prevents enqueue inversion. The lock here is
/// held across a map lookup and an addition, and never across an await.
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
    #[cfg(test)]
    pub(crate) fn take(&self, id: &SessionId, floor: u64, count: u64) -> u64 {
        self.try_take(id, floor, count)
            .expect("test sequence allocation stays within u64")
    }

    /// Checked form used by the durable lane.
    pub(crate) fn try_take(
        &self,
        id: &SessionId,
        floor: u64,
        count: u64,
    ) -> Result<u64, StoreError> {
        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        let last = issued.entry(id.clone()).or_insert(0);
        *last = (*last).max(floor);
        let first = last
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;
        *last = last
            .checked_add(count)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;
        Ok(first)
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

    /// The currently issued tail, without creating an allocator entry.
    pub(crate) fn current(&self, id: &SessionId) -> Option<u64> {
        self.issued
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .copied()
    }

    /// Return an abandoned reservation to this session's lane.
    ///
    /// The lane serializes this with allocation and shifts every queued unit by
    /// the same amount. Lowering is therefore safe here even though ordinary
    /// observations may only move the allocator forward.
    pub(crate) fn rewind(&self, id: &SessionId, count: u64) {
        let mut issued = self.issued.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = issued.get_mut(id) {
            *last = last.saturating_sub(count);
        }
    }

    /// Reconcile every reservation in a lane to a store-authoritative tail.
    ///
    /// Called only while the lane lock excludes allocation. Unlike `issued`,
    /// this may move backward when a missing local write is being closed.
    pub(crate) fn reconcile(&self, id: &SessionId, tail: u64) {
        self.issued
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), tail);
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

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.issued.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[cfg(test)]
#[path = "sequence_tests.rs"]
mod sequence_tests;
