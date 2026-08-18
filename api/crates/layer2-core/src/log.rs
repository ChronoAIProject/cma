//! Writing to a session's durable log.
//!
//! Two steps, deliberately apart. An event is buffered against the live entry
//! by whoever noticed it, and made durable later; splitting them is what lets
//! a caller holding the entries lock record something without doing I/O while
//! it holds that lock.
//!
//! The log is the record. `API_Overview.md` makes it the source of truth and
//! the thing a client resumes from, so an event that is not written here is
//! not recoverable by any route — the turn ran and no interface can say what
//! it did. That is the standard everything in this file is held to.

use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::service::{SessionEntry, SessionService};
use crate::store::{SessionRecord, StoreError};

/// How many times a refused batch is re-numbered before it is given up on.
///
/// Two, because the first retry reads where the log has actually reached and
/// allocates above it; a second collision means another writer moved again in
/// that window, and a third would be a loop rather than a recovery. Giving up
/// is loud — the alternative is retrying forever against a log we are
/// evidently not the only writer of.
const APPEND_ATTEMPTS: usize = 3;

impl SessionService {
    /// Write events durably, re-numbering anything the store refuses.
    ///
    /// A sequence already taken is the only failure here worth another
    /// attempt, and it is the one that needs a *different* attempt: the batch
    /// cannot be retried as it stands, because the number it claims belongs to
    /// something else. Retried unchanged it fails identically forever;
    /// discarded — which is what used to happen — a turn that had already run
    /// leaves nothing in the log for any interface to report.
    ///
    /// Only the refused remainder is re-numbered. The store reports how many
    /// of the batch it accepted before it stopped, and writing those again
    /// under fresh sequences would put the same event in the log twice, under
    /// two numbers, with nothing to reject it.
    ///
    /// The new numbers are allocated above where the log has actually reached
    /// rather than above what this process last issued, because a collision is
    /// evidence that something else is writing and our position is behind.
    pub(crate) async fn append_durably(
        &self,
        id: &SessionId,
        mut events: Vec<Event>,
    ) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        for attempt in 1..=APPEND_ATTEMPTS {
            match self.store.append_events(&events).await {
                Ok(()) => return Ok(()),
                Err(StoreError::SequenceTaken { written, total }) => {
                    // A duplicate is refused at a position inside the batch,
                    // so a store reporting that it wrote all of one it also
                    // refused is contradicting itself. Believing it would
                    // leave nothing to re-number and report success over a
                    // batch that is not in the log — which is the silence this
                    // whole path exists to end.
                    if written >= events.len() {
                        tracing::error!(
                            session_id = %id,
                            written,
                            total,
                            held = events.len(),
                            "the store refused a batch it says it wrote in full; \
                             these events are not in the log"
                        );
                        return Err(StoreError::SequenceTaken { written, total });
                    }
                    events.drain(..written);
                    let reached = self.log_reaches(id).await;
                    let first = self.sequences.take(id, reached, events.len() as u64);
                    for (n, event) in events.iter_mut().enumerate() {
                        event.seq = first + n as u64;
                    }
                    tracing::warn!(
                        session_id = %id,
                        attempt,
                        accepted = written,
                        of = total,
                        renumbered_from = first,
                        "a sequence this session already had; re-numbering the rest"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        session_id = %id,
                        error = %e,
                        lost = events.len(),
                        from_seq = events.first().map(|e| e.seq),
                        to_seq = events.last().map(|e| e.seq),
                        "cannot persist events"
                    );
                    return Err(e);
                }
            }
        }
        tracing::error!(
            session_id = %id,
            attempts = APPEND_ATTEMPTS,
            lost = events.len(),
            from_seq = events.first().map(|e| e.seq),
            to_seq = events.last().map(|e| e.seq),
            "gave up numbering these events; they are not in the log and cannot be recovered"
        );
        Err(StoreError::SequenceTaken {
            written: 0,
            total: events.len(),
        })
    }

    /// The highest sequence the durable log actually holds for a session.
    ///
    /// Read from the events themselves rather than from the session record:
    /// the record is written whole by several paths, so it can be left behind
    /// the log, and it is exactly that staleness a collision has just proved.
    ///
    /// Only events above what this process has issued are read, so the usual
    /// answer costs one bounded query returning nothing. A store that cannot
    /// be read falls back to our own position — no worse than not asking, and
    /// the next attempt is what covers it.
    async fn log_reaches(&self, id: &SessionId) -> u64 {
        let issued = self.sequences.issued(id, 0);
        match self.store.events_since(id, issued).await {
            Ok(found) => found.iter().map(|e| e.seq).max().unwrap_or(issued),
            Err(e) => {
                tracing::warn!(
                    session_id = %id,
                    error = %e,
                    "cannot read where this session's log reached; numbering from what this \
                     process has issued"
                );
                issued
            }
        }
    }

    /// Persist a session's current state and any events buffered against it.
    ///
    /// Called after every state change. Failures are logged rather than
    /// propagated where the caller has already acted: the runtime really did
    /// change, and losing the record is a storage problem to surface, not a
    /// reason to tell the caller their action failed.
    pub(crate) async fn flush(&self, id: &SessionId) {
        let (record, events) = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            let events = std::mem::take(&mut entry.events);
            let mut session = entry.session.clone();
            // The position of the log, not of this entry's copy of it. Other
            // paths number events for the same session off the record, and a
            // record written behind them is what a restarted process would
            // start from — too low, onto sequences already taken.
            session.last_seq = entry.sequences.issued(id, session.last_seq);
            let mut record = SessionRecord::new(session);
            record.runtime_id = entry.runtime.as_ref().map(|h| h.0.clone());
            record.context_ref = entry.context_ref.clone();
            (record, events)
        };
        if let Err(e) = self.store.upsert(&record).await {
            tracing::error!(session_id = %id, error = %e, "cannot persist session");
        }
        // Already logged in detail by the writer, including what was lost if
        // anything was. Nothing above this has acted on the events, so there
        // is nothing here to undo.
        let _ = self.append_durably(id, events).await;
    }
}

/// Append a durable event, allocating the next per-session sequence.
///
/// Buffers on the entry; `flush` is what makes it durable. Split so a caller
/// holding the entries lock does not perform I/O while holding it.
pub(crate) fn append(entry: &mut SessionEntry, kind: EventKind) {
    let seq = entry
        .sequences
        .take(&entry.session.id, entry.session.last_seq, 1);
    entry.session.last_seq = seq;
    entry.events.push(Event {
        session_id: entry.session.id.clone(),
        seq,
        kind,
    });
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;
