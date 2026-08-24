//! Ordered, cancellation-safe session persistence.
//!
//! Every state writer places one record snapshot and its own events into a
//! session-scoped FIFO. A detached worker owns the write from that point on, so
//! cancelling the caller cannot strand the only copy of a drained batch. The
//! store remains the cross-process authority: local lanes order one process,
//! while store revisions, atomic transactions, and contiguous appends arbitrate
//! replicas.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::sequence::Sequences;
use crate::service::{SessionEntry, SessionService};
use crate::store::{SessionRecord, SessionStore, StoreError};

const FIRST_RETRY_DELAY: Duration = Duration::from_millis(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
const REPORT_AFTER_ATTEMPTS: usize = 4;

#[derive(Clone, Copy)]
enum CommitMode {
    Create,
    /// A snapshot read independently from the durable store.
    UpdateExact,
    /// A live-entry snapshot ordered behind every earlier snapshot in this lane.
    UpdateInheriting,
}

enum PendingOperation {
    Commit {
        write_id: String,
        mode: CommitMode,
        record: Box<SessionRecord>,
        events: Vec<Event>,
        complete: oneshot::Sender<Result<SessionRecord, StoreError>>,
    },
    Delete {
        complete: oneshot::Sender<Result<(), StoreError>>,
    },
    ObserveRevision(u64),
}

struct EnqueuedCommit {
    complete: oneshot::Receiver<Result<SessionRecord, StoreError>>,
    last_seq: u64,
}

#[derive(Default)]
struct LaneState {
    running: bool,
    // Deliberately lossless rather than hard-capped. Once live state has
    // emitted a semantic event, rejecting it at this boundary would make the
    // event unrecoverable. Memory therefore tracks uncommitted history during
    // a store outage; session-key cardinality is bounded separately by the
    // weak registry, and upstream runtime channels retain their own caps.
    queued: VecDeque<PendingOperation>,
}

/// One process's FIFO for a session.
pub(crate) struct SessionLog {
    id: SessionId,
    store: Arc<dyn SessionStore>,
    sequences: Arc<Sequences>,
    revision: AtomicU64,
    state: StdMutex<LaneState>,
}

impl SessionLog {
    fn new(id: SessionId, store: Arc<dyn SessionStore>, sequences: Arc<Sequences>) -> Self {
        Self {
            id,
            store,
            sequences,
            revision: AtomicU64::new(0),
            state: StdMutex::new(LaneState::default()),
        }
    }

    pub(crate) fn observe_revision(self: &Arc<Self>, revision: u64) {
        self.enqueue(PendingOperation::ObserveRevision(revision));
    }

    fn enqueue_commit(
        self: &Arc<Self>,
        mode: CommitMode,
        record: SessionRecord,
        events: Vec<Event>,
    ) -> EnqueuedCommit {
        self.enqueue_commit_with(mode, record, events, |_| {})
    }

    fn enqueue_commit_with<F>(
        self: &Arc<Self>,
        mode: CommitMode,
        mut record: SessionRecord,
        mut events: Vec<Event>,
        after_allocation: F,
    ) -> EnqueuedCommit
    where
        F: FnOnce(u64),
    {
        let (complete, receiver) = oneshot::channel();
        if record.session.id != self.id || events.iter().any(|event| event.session_id != self.id) {
            let _ = complete.send(Err(StoreError::Malformed(
                "one durable session write named more than one session".into(),
            )));
            return EnqueuedCommit {
                complete: receiver,
                last_seq: record.session.last_seq,
            };
        }
        let appended_at = crate::clock::now_rfc3339();
        for event in &mut events {
            if event.appended_at.is_empty() {
                event.appended_at = appended_at.clone();
            }
        }

        // Sequence allocation and FIFO insertion are one critical section.
        // No producer can reserve N, pause, and let N+1 become the queue head.
        // Lock order is lane state -> sequence allocator; nothing takes those
        // locks in the opposite order.
        let (start, last_seq) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !events.is_empty() {
                let count = u64::try_from(events.len()).unwrap_or(u64::MAX);
                let first = match self
                    .sequences
                    .try_take(&self.id, record.session.last_seq, count)
                {
                    Ok(first) => first,
                    Err(error) => {
                        let _ = complete.send(Err(error));
                        return EnqueuedCommit {
                            complete: receiver,
                            last_seq: record.session.last_seq,
                        };
                    }
                };
                if let Err(error) = renumber(&self.id, &mut events, first) {
                    let _ = complete.send(Err(error));
                    return EnqueuedCommit {
                        complete: receiver,
                        last_seq: record.session.last_seq,
                    };
                }
                record.session.last_seq = events
                    .last()
                    .map_or(record.session.last_seq, |event| event.seq);
            }
            let last_seq = record.session.last_seq;
            after_allocation(last_seq);
            state.queued.push_back(PendingOperation::Commit {
                write_id: format!("sesw_{}", uuid::Uuid::new_v4().simple()),
                mode,
                record: Box::new(record),
                events,
                complete,
            });
            let start = if state.running {
                false
            } else {
                state.running = true;
                true
            };
            (start, last_seq)
        };
        if start {
            let lane = Arc::clone(self);
            tokio::spawn(async move { lane.run().await });
        }
        EnqueuedCommit {
            complete: receiver,
            last_seq,
        }
    }

    fn enqueue_delete(self: &Arc<Self>) -> oneshot::Receiver<Result<(), StoreError>> {
        let (complete, receiver) = oneshot::channel();
        self.enqueue(PendingOperation::Delete { complete });
        receiver
    }

    fn enqueue(self: &Arc<Self>, operation: PendingOperation) {
        let start = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.queued.push_back(operation);
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };
        if start {
            let lane = Arc::clone(self);
            tokio::spawn(async move { lane.run().await });
        }
    }

    async fn run(self: Arc<Self>) {
        loop {
            let operation = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                match state.queued.pop_front() {
                    Some(operation) => operation,
                    None => {
                        state.running = false;
                        return;
                    }
                }
            };

            match operation {
                PendingOperation::Commit {
                    write_id,
                    mode,
                    mut record,
                    mut events,
                    complete,
                } => {
                    let mut complete = Some(complete);
                    let retain_after_cancellation = !matches!(mode, CommitMode::Create);
                    let result = self
                        .commit(
                            &write_id,
                            mode,
                            &mut record,
                            &mut events,
                            &mut complete,
                            retain_after_cancellation,
                        )
                        .await
                        .map(|()| *record);
                    if let Some(complete) = complete {
                        let _ = complete.send(result);
                    }
                }
                PendingOperation::Delete { complete } => {
                    let result = self.delete().await;
                    let _ = complete.send(result);
                }
                PendingOperation::ObserveRevision(revision) => {
                    self.revision.fetch_max(revision, Ordering::AcqRel);
                }
            }
        }
    }

    async fn commit(
        &self,
        write_id: &str,
        mode: CommitMode,
        record: &mut SessionRecord,
        events: &mut [Event],
        complete: &mut Option<oneshot::Sender<Result<SessionRecord, StoreError>>>,
        retain_after_cancellation: bool,
    ) -> Result<(), StoreError> {
        if matches!(mode, CommitMode::UpdateInheriting) {
            record.revision = record.revision.max(self.revision.load(Ordering::Acquire));
        }
        let mut attempt = 0_usize;
        // Collision recovery is forward progress, not an outage retry, and
        // must not consume or skip the caller's four-unavailable boundary.
        let mut unavailable_attempt = 0_usize;
        loop {
            attempt = attempt.saturating_add(1);
            if let Some(last) = events.last() {
                record.session.last_seq = last.seq;
            }

            let result = match mode {
                CommitMode::Create => {
                    self.store
                        .create_session_idempotent(write_id, record, events)
                        .await
                }
                CommitMode::UpdateExact | CommitMode::UpdateInheriting => {
                    self.store
                        .commit_session_idempotent(write_id, record, events)
                        .await
                }
            };
            match result {
                Ok(revision) => {
                    self.revision.fetch_max(revision, Ordering::AcqRel);
                    record.revision = revision;
                    if let Some(last) = events.last() {
                        self.sequences.issued(&self.id, last.seq);
                    }
                    return Ok(());
                }
                Err(StoreError::SequenceTaken { expected, .. }) if !events.is_empty() => {
                    if self.durably_terminated().await {
                        return Err(StoreError::SessionTerminated {
                            id: self.id.clone(),
                        });
                    }
                    renumber(&self.id, events, expected)?;
                    if let Some(last) = events.last() {
                        record.session.last_seq = last.seq;
                        self.sequences.issued(&self.id, last.seq);
                    }
                    tracing::warn!(
                        session_id = %self.id,
                        attempt,
                        from_seq = expected,
                        "a replica advanced the durable event tail; rebasing this write unit"
                    );
                }
                Err(StoreError::NonContiguousAppend { expected, .. }) if !events.is_empty() => {
                    if self.durably_terminated().await {
                        return Err(StoreError::SessionTerminated {
                            id: self.id.clone(),
                        });
                    }
                    renumber(&self.id, events, expected)?;
                    if let Some(last) = events.last() {
                        record.session.last_seq = last.seq;
                    }
                    tracing::warn!(
                        session_id = %self.id,
                        attempt,
                        from_seq = expected,
                        "an earlier local write stopped; closing its uncommitted sequence gap"
                    );
                }
                Err(StoreError::RevisionConflict { .. })
                    if !events.is_empty() && !matches!(mode, CommitMode::Create) =>
                {
                    match self.rebase_conflicted_write(record, events).await {
                        Ok(()) => {
                            tracing::warn!(
                                session_id = %self.id,
                                attempt,
                                revision = record.revision,
                                "another replica changed the session; rebasing this semantic write"
                            );
                        }
                        Err(error @ StoreError::Unavailable(_)) => {
                            unavailable_attempt = unavailable_attempt.saturating_add(1);
                            self.retry_or_stop(
                                error,
                                unavailable_attempt,
                                complete,
                                retain_after_cancellation,
                            )
                            .await?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error @ StoreError::Unavailable(_)) => {
                    unavailable_attempt = unavailable_attempt.saturating_add(1);
                    self.retry_or_stop(
                        error,
                        unavailable_attempt,
                        complete,
                        retain_after_cancellation,
                    )
                    .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn rebase_conflicted_write(
        &self,
        record: &mut SessionRecord,
        events: &mut [Event],
    ) -> Result<(), StoreError> {
        let durable =
            self.store
                .get(&self.id)
                .await?
                .ok_or_else(|| StoreError::SessionDeleted {
                    id: self.id.clone(),
                })?;
        if durable.session.status.is_terminal() {
            return Err(StoreError::SessionTerminated {
                id: self.id.clone(),
            });
        }

        let intended = record.clone();
        *record = rebase_semantic_record(&intended, durable, events);
        let first = record.session.last_seq.checked_add(1).ok_or_else(|| {
            StoreError::Malformed(format!("event sequence overflow for {}", self.id))
        })?;
        renumber(&self.id, events, first)?;
        if let Some(last) = events.last() {
            record.session.last_seq = last.seq;
            self.sequences.issued(&self.id, last.seq);
        }
        self.revision.fetch_max(record.revision, Ordering::AcqRel);
        Ok(())
    }

    async fn retry_or_stop(
        &self,
        error: StoreError,
        unavailable_attempt: usize,
        complete: &mut Option<oneshot::Sender<Result<SessionRecord, StoreError>>>,
        retain_after_cancellation: bool,
    ) -> Result<(), StoreError> {
        if unavailable_attempt == 1 || unavailable_attempt.is_power_of_two() {
            tracing::warn!(
                session_id = %self.id,
                attempt = unavailable_attempt,
                error = %error,
                "durable session write retained by its retry worker"
            );
        }
        if unavailable_attempt == REPORT_AFTER_ATTEMPTS {
            if let Some(reporter) = complete.take() {
                if reporter.send(Err(error.clone())).is_ok() {
                    // Delivery, rather than a racy `is_closed` observation,
                    // is the boundary: after an explicit failure the worker
                    // no longer owns permission to commit this operation.
                    return Err(error);
                }
            }
            if !retain_after_cancellation {
                return Err(error);
            }
        }
        retry_delay(unavailable_attempt).await;
        Ok(())
    }

    async fn durably_terminated(&self) -> bool {
        matches!(
            self.store.get(&self.id).await,
            Ok(Some(record)) if record.session.status.is_terminal()
        )
    }

    async fn delete(&self) -> Result<(), StoreError> {
        let mut attempt = 0_usize;
        loop {
            attempt = attempt.saturating_add(1);
            match self.store.delete(&self.id).await {
                Ok(()) => return Ok(()),
                Err(error @ StoreError::Unavailable(_)) => {
                    if attempt == 1 || attempt.is_power_of_two() {
                        tracing::warn!(
                            session_id = %self.id,
                            attempt,
                            error = %error,
                            "durable session deletion retained by its retry worker"
                        );
                    }
                    if attempt == REPORT_AFTER_ATTEMPTS {
                        return Err(error);
                    }
                    retry_delay(attempt).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(test)]
    fn queued(&self) -> usize {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.queued.len() + usize::from(state.running)
    }
}

/// Weak registry of currently active session lanes.
///
/// A live entry, caller, or worker owns its lane strongly. Dead weak entries are
/// pruned on every lookup, so touching one-off sessions cannot grow this map
/// without bound.
pub(crate) struct EventLogs {
    held: StdMutex<HashMap<SessionId, Weak<SessionLog>>>,
    store: Arc<dyn SessionStore>,
    sequences: Arc<Sequences>,
}

impl EventLogs {
    pub(crate) fn new(store: Arc<dyn SessionStore>, sequences: Arc<Sequences>) -> Self {
        Self {
            held: StdMutex::new(HashMap::new()),
            store,
            sequences,
        }
    }

    pub(crate) fn lane(&self, id: &SessionId) -> Arc<SessionLog> {
        let mut held = self.held.lock().unwrap_or_else(|error| error.into_inner());
        held.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = held.get(id).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(SessionLog::new(
            id.clone(),
            Arc::clone(&self.store),
            Arc::clone(&self.sequences),
        ));
        held.insert(id.clone(), Arc::downgrade(&lane));
        lane
    }

    #[cfg(test)]
    fn registry_len(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl SessionService {
    pub(crate) async fn create_durably(
        &self,
        record: &mut SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        let result = lane
            .enqueue_commit(CommitMode::Create, record.clone(), events)
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    pub(crate) async fn persist_durably(
        &self,
        record: &mut SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        let result = lane
            .enqueue_commit(CommitMode::UpdateExact, record.clone(), events)
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Delete a session and its history behind every local write already queued.
    pub(crate) async fn delete_durably(&self, id: &SessionId) -> Result<(), StoreError> {
        self.event_logs
            .lane(id)
            .enqueue_delete()
            .await
            .map_err(|_| StoreError::Unavailable("session delete worker stopped".into()))?
    }

    /// Persist the current live record after all event snapshots queued before it.
    pub(crate) async fn flush(&self, id: &SessionId) {
        let receiver = {
            let entries = self.entries.read().await;
            let Some(entry) = entries.get(id) else {
                return;
            };
            // Queue before releasing the entry snapshot: a later writer cannot
            // enqueue newer state ahead of this record-only barrier.
            entry
                .event_log
                .enqueue_commit(
                    CommitMode::UpdateInheriting,
                    record_from_entry(entry),
                    Vec::new(),
                )
                .complete
        };
        if let Err(error) = receiver
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))
            .and_then(|result| result.map(|_| ()))
        {
            tracing::error!(session_id = %id, error = %error, "cannot persist session");
        }
    }
}

fn record_from_entry(entry: &SessionEntry) -> SessionRecord {
    let mut record = SessionRecord::new(entry.session.clone());
    record.runtime_id = entry.runtime.as_ref().map(|handle| handle.0.clone());
    record.context_ref = entry.context_ref.clone();
    record.revision = entry.event_log.revision.load(Ordering::Acquire);
    record
}

/// Reapply the record effects represented by an event unit to a fresh durable
/// snapshot. Event-only writes keep fields changed by the competing replica;
/// state-changing events carry their corresponding record intent forward.
fn rebase_semantic_record(
    intended: &SessionRecord,
    mut durable: SessionRecord,
    events: &[Event],
) -> SessionRecord {
    for event in events {
        match &event.kind {
            EventKind::StatusChanged {
                status,
                stop_reason,
                ..
            } => {
                durable.session.status = *status;
                durable.session.stop_reason = *stop_reason;
                durable.session.recovery_state = intended.session.recovery_state;
                if !status.wants_runtime() {
                    durable.runtime_id = intended.runtime_id.clone();
                }
            }
            EventKind::TerminalResized { .. } => {
                durable.session.geometry = intended.session.geometry;
            }
            EventKind::PlaneChanged { .. } => {
                durable.session.options.mode = intended.session.options.mode;
                durable.session.capabilities = intended.session.capabilities;
                durable.session.runtime_version = intended.session.runtime_version.clone();
                durable.session.rotations = intended.session.rotations;
                durable.session.rotated_at = intended.session.rotated_at.clone();
                durable.runtime_id = intended.runtime_id.clone();
            }
            EventKind::RuntimeLost { .. } => {
                durable.runtime_id = intended.runtime_id.clone();
                durable.session.recovery_state = intended.session.recovery_state;
            }
            _ => {}
        }
    }
    durable
}

fn renumber(id: &SessionId, events: &mut [Event], first: u64) -> Result<(), StoreError> {
    for (offset, event) in events.iter_mut().enumerate() {
        event.seq = first
            .checked_add(offset as u64)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;
    }
    Ok(())
}

async fn retry_delay(attempt: usize) {
    let shift = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let multiplier = 1_u32.checked_shl(shift).unwrap_or(u32::MAX);
    tokio::time::sleep(
        FIRST_RETRY_DELAY
            .saturating_mul(multiplier)
            .min(MAX_RETRY_DELAY),
    )
    .await;
}

/// Record an event and immediately hand its exact state snapshot to the lane.
pub(crate) fn append(entry: &mut SessionEntry, kind: EventKind) {
    let event = Event {
        session_id: entry.session.id.clone(),
        seq: 0,
        kind,
        appended_at: String::new(),
    };
    let record = record_from_entry(entry);
    let enqueued =
        entry
            .event_log
            .enqueue_commit(CommitMode::UpdateInheriting, record, vec![event]);
    entry.session.last_seq = enqueued.last_seq;
    drop(enqueued.complete);
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;

#[cfg(test)]
#[path = "flush_tests.rs"]
mod flush_tests;
