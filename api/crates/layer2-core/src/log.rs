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
use crate::store::SessionRecord;

impl SessionService {
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
        if let Err(e) = self.store.append_events(&events).await {
            tracing::error!(session_id = %id, error = %e, "cannot persist events");
        }
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
