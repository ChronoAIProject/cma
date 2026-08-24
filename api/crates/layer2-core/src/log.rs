//! Ordered, cancellation-safe session persistence.
//!
//! Every state writer places one record snapshot and its own events into a
//! session-scoped FIFO. A detached worker owns retryable writes from that point
//! on. An outage failure ends an update only when the receiver acknowledges it
//! in the poll that observes it; cancellation before then leaves the worker in
//! charge. The store remains the cross-process authority: local lanes order one
//! process, while store revisions, atomic transactions, and contiguous appends
//! arbitrate replicas.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::task::{Context, Poll};
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
        change: Option<Box<RecordChange>>,
        events: Vec<Event>,
        complete: oneshot::Sender<Result<CommitDelivery<SessionRecord>, StoreError>>,
        observed: oneshot::Receiver<()>,
        retained_outcome: Option<oneshot::Sender<Result<SessionRecord, StoreError>>>,
    },
    Delete {
        complete: oneshot::Sender<Result<(), StoreError>>,
    },
    Fence {
        complete: oneshot::Sender<()>,
    },
    ObserveRecord(Box<SessionRecord>),
    #[cfg(test)]
    ObserveRevision(u64),
}

/// One live snapshot's change from the snapshot immediately before it.
///
/// The predecessor is retained beside the pending operation's intended record.
/// A revision is only a compare token; it cannot prove that a queued full record
/// contains fields a competing replica committed under that revision.
/// Reapplying this change to the latest durable record preserves fields this
/// process did not change and detects two replicas changing the same field
/// differently.
struct RecordChange {
    before: Box<SessionRecord>,
    fields: RecordFields,
}

#[derive(Clone, Copy, Default)]
struct RecordFields {
    configuration: bool,
    status: bool,
    geometry: bool,
    plane: bool,
    runtime_loss: bool,
    runtime: bool,
    context: bool,
}

struct EnqueuedCommit {
    complete: CommitReceiver,
    last_seq: u64,
}

type DeliveryCleanupFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type DeliveryCleanup<T> = Box<dyn FnOnce(T) -> DeliveryCleanupFuture + Send>;

/// A successful result that still owns its cancellation cleanup until polled.
///
/// Sending through a oneshot is not observation: the receiver can be canceled
/// after `send` succeeds but before it polls the value. Keeping cleanup in the
/// value itself closes that window. Polling extracts the output and disarms the
/// cleanup; dropping an unread delivery launches it.
struct CommitDelivery<T> {
    output: Option<T>,
    cleanup: Option<DeliveryCleanup<T>>,
}

impl<T> CommitDelivery<T> {
    fn plain(output: T) -> Self {
        Self {
            output: Some(output),
            cleanup: None,
        }
    }

    fn retained<H, U, UFut>(output: T, ownership: H, cleanup: U) -> Self
    where
        T: Send + 'static,
        H: Send + 'static,
        U: FnOnce(T, H) -> UFut + Send + 'static,
        UFut: Future<Output = ()> + Send + 'static,
    {
        Self {
            output: Some(output),
            cleanup: Some(Box::new(move |output| Box::pin(cleanup(output, ownership)))),
        }
    }

    fn observed(mut self) -> T {
        let output = self
            .output
            .take()
            .expect("a successful commit delivery is observed at most once");
        drop(self.cleanup.take());
        output
    }
}

impl<T> Drop for CommitDelivery<T> {
    fn drop(&mut self) {
        let (Some(output), Some(cleanup)) = (self.output.take(), self.cleanup.take()) else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(cleanup(output));
            }
            Err(error) => {
                tracing::error!(%error, "cannot run cleanup for an unobserved commit delivery");
            }
        }
    }
}

/// Reports that an explicit failure was actually observed in the caller's
/// poll, rather than merely placed in a channel it canceled without reading.
pub(crate) struct CommitReceiver<T = SessionRecord> {
    complete: oneshot::Receiver<Result<CommitDelivery<T>, StoreError>>,
    acknowledge_error: Option<oneshot::Sender<()>>,
}

impl<T> Future for CommitReceiver<T> {
    type Output = Result<Result<T, StoreError>, oneshot::error::RecvError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.complete).poll(context) {
            Poll::Ready(Ok(Ok(delivery))) => Poll::Ready(Ok(Ok(delivery.observed()))),
            Poll::Ready(Ok(Err(error))) => {
                if let Some(acknowledge) = self.acknowledge_error.take() {
                    let _ = acknowledge.send(());
                }
                Poll::Ready(Ok(Err(error)))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> CommitReceiver<T> {
    pub(crate) async fn committed(self) -> Result<T, StoreError> {
        self.await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))?
    }
}

/// Keep a proposal's post-commit work alive independently of its waiter.
///
/// The ordinary completion channel may report an outage after four attempts,
/// but only its external receiver is allowed to acknowledge that report. If
/// the receiver is cancelled, the lane retains the write and this task keeps
/// both the continuation and its ownership guard until eventual success.
fn retain_commit<T, H, F, Fut, R, RFut, U, UFut>(
    id: SessionId,
    commit: CommitReceiver,
    retained_outcome: oneshot::Receiver<Result<SessionRecord, StoreError>>,
    continuation: F,
    on_rejected: R,
    on_undelivered: U,
) -> CommitReceiver<T>
where
    T: Send + 'static,
    H: Send + 'static,
    F: FnOnce(SessionRecord) -> Fut + Send + 'static,
    Fut: Future<Output = (T, H)> + Send + 'static,
    R: FnOnce(StoreError) -> RFut + Send + 'static,
    RFut: Future<Output = ()> + Send + 'static,
    U: FnOnce(T, H) -> UFut + Send + 'static,
    UFut: Future<Output = ()> + Send + 'static,
{
    let CommitReceiver {
        complete: durable,
        acknowledge_error: acknowledge_durable_error,
    } = commit;
    let (report, complete) = oneshot::channel();
    let (acknowledge_error, error_observed) = oneshot::channel();
    tokio::spawn(async move {
        let mut report = Some(report);
        let mut error_was_observed = false;
        match durable.await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                let delivered = report
                    .take()
                    .expect("a retained commit reports at most once")
                    .send(Err(error))
                    .is_ok();
                if delivered && error_observed.await.is_ok() {
                    error_was_observed = true;
                    if let Some(acknowledge) = acknowledge_durable_error {
                        let _ = acknowledge.send(());
                    }
                } else {
                    // Dropping this sender is the signal that the report was not
                    // externally observed. The lane then keeps retrying.
                    drop(acknowledge_durable_error);
                }
            }
            Err(_) => {
                let error = StoreError::Unavailable("session write worker stopped".into());
                let _ = report
                    .take()
                    .expect("a retained commit reports at most once")
                    .send(Err(error.clone()));
                on_rejected(error).await;
                return;
            }
        }

        match retained_outcome.await {
            Ok(Ok(record)) => {
                let (output, ownership) = continuation(record).await;
                if let Some(report) = report {
                    let _ = report.send(Ok(CommitDelivery::retained(
                        output,
                        ownership,
                        on_undelivered,
                    )));
                } else {
                    on_undelivered(output, ownership).await;
                }
            }
            Ok(Err(error)) => {
                on_rejected(error.clone()).await;
                if let Some(report) = report {
                    let _ = report.send(Err(error));
                } else if !error_was_observed {
                    tracing::error!(
                        session_id = %id,
                        error = %error,
                        "a detached durable session proposal failed"
                    );
                }
            }
            Err(_) => {
                let error = StoreError::Unavailable("session write worker stopped".into());
                on_rejected(error.clone()).await;
                if let Some(report) = report {
                    let _ = report.send(Err(error));
                } else if !error_was_observed {
                    tracing::error!(
                        session_id = %id,
                        error = %error,
                        "a detached durable session proposal stopped without an outcome"
                    );
                }
            }
        }
    });
    CommitReceiver {
        complete,
        acknowledge_error: Some(acknowledge_error),
    }
}

pub(crate) async fn ignore_rejected(_error: StoreError) {}

pub(crate) async fn drop_undelivered<T, H>(_output: T, _ownership: H) {}

struct CommitReporter {
    complete: Option<oneshot::Sender<Result<CommitDelivery<SessionRecord>, StoreError>>>,
    observed: Option<oneshot::Receiver<()>>,
    retain_after_cancellation: bool,
    retained_outcome: Option<oneshot::Sender<Result<SessionRecord, StoreError>>>,
}

#[derive(Default)]
struct LaneState {
    running: bool,
    /// The newest authoritative record this worker has observed or committed.
    durable: Option<Box<SessionRecord>>,
    /// The raw snapshot at the tail of the local FIFO.
    ///
    /// Kept separate from `durable`: after a cross-replica rebase the live entry
    /// still produces snapshots in its own local lineage. Successor changes must
    /// be measured against that lineage, then applied to the rebased record.
    local_tail: Option<Box<SessionRecord>>,
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
    #[cfg(test)]
    failure_delivered: tokio::sync::Notify,
}

impl SessionLog {
    fn new(id: SessionId, store: Arc<dyn SessionStore>, sequences: Arc<Sequences>) -> Self {
        Self {
            id,
            store,
            sequences,
            revision: AtomicU64::new(0),
            state: StdMutex::new(LaneState::default()),
            #[cfg(test)]
            failure_delivered: tokio::sync::Notify::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_revision(self: &Arc<Self>, revision: u64) {
        self.enqueue(PendingOperation::ObserveRevision(revision));
    }

    /// Place a durable snapshot in the FIFO before exposing a live entry built
    /// from it.
    ///
    /// `local_tail` changes at enqueue time so an immediately appended event can
    /// describe its change from this exact snapshot. The worker observes the
    /// durable side in FIFO order, after any older write already in flight.
    pub(crate) fn observe_record(self: &Arc<Self>, record: &SessionRecord) {
        debug_assert_eq!(record.session.id, self.id);
        let start = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.local_tail = Some(Box::new(record.clone()));
            state
                .queued
                .push_back(PendingOperation::ObserveRecord(Box::new(record.clone())));
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

    /// Seed the local allocator from a durable record before exposing a live
    /// entry backed by this lane.
    pub(crate) fn observe_tail(&self, tail: u64) {
        let _state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        self.sequences.issued(&self.id, tail);
    }

    /// The lane-authoritative tail for a live entry snapshot.
    fn known_tail(&self, fallback: u64) -> u64 {
        let _state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        self.sequences.current(&self.id).unwrap_or(fallback)
    }

    /// Bring a live entry's cached tail into line while excluding a concurrent
    /// reservation rebase.
    pub(crate) fn synchronize_tail(&self, tail: &mut u64) {
        let _state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(current) = self.sequences.current(&self.id) {
            *tail = current;
        }
    }

    fn enqueue_commit(
        self: &Arc<Self>,
        mode: CommitMode,
        record: SessionRecord,
        events: Vec<Event>,
    ) -> EnqueuedCommit {
        self.enqueue_commit_with(mode, record, events, None, |_| {})
    }

    fn enqueue_commit_with<F>(
        self: &Arc<Self>,
        mode: CommitMode,
        record: SessionRecord,
        events: Vec<Event>,
        owned_fields: Option<RecordFields>,
        after_allocation: F,
    ) -> EnqueuedCommit
    where
        F: FnOnce(u64),
    {
        self.enqueue_commit_inner(mode, record, events, owned_fields, None, after_allocation)
    }

    fn enqueue_retained_commit<F>(
        self: &Arc<Self>,
        mode: CommitMode,
        record: SessionRecord,
        events: Vec<Event>,
        owned_fields: Option<RecordFields>,
        after_allocation: F,
    ) -> (
        EnqueuedCommit,
        oneshot::Receiver<Result<SessionRecord, StoreError>>,
    )
    where
        F: FnOnce(u64),
    {
        let (retained_outcome, outcome) = oneshot::channel();
        (
            self.enqueue_commit_inner(
                mode,
                record,
                events,
                owned_fields,
                Some(retained_outcome),
                after_allocation,
            ),
            outcome,
        )
    }

    fn enqueue_commit_inner<F>(
        self: &Arc<Self>,
        mode: CommitMode,
        mut record: SessionRecord,
        mut events: Vec<Event>,
        owned_fields: Option<RecordFields>,
        retained_outcome: Option<oneshot::Sender<Result<SessionRecord, StoreError>>>,
        after_allocation: F,
    ) -> EnqueuedCommit
    where
        F: FnOnce(u64),
    {
        let (complete, receiver) = oneshot::channel();
        let (acknowledge_error, observed) = oneshot::channel();
        if record.session.id != self.id || events.iter().any(|event| event.session_id != self.id) {
            let _ = complete.send(Err(StoreError::Malformed(
                "one durable session write named more than one session".into(),
            )));
            return EnqueuedCommit {
                complete: CommitReceiver {
                    complete: receiver,
                    acknowledge_error: Some(acknowledge_error),
                },
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
            if matches!(mode, CommitMode::UpdateInheriting) {
                record.session.last_seq = self
                    .sequences
                    .current(&self.id)
                    .unwrap_or(record.session.last_seq);
            }
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
                            complete: CommitReceiver {
                                complete: receiver,
                                acknowledge_error: Some(acknowledge_error),
                            },
                            last_seq: record.session.last_seq,
                        };
                    }
                };
                if let Err(error) = renumber(&self.id, &mut events, first) {
                    let _ = complete.send(Err(error));
                    return EnqueuedCommit {
                        complete: CommitReceiver {
                            complete: receiver,
                            acknowledge_error: Some(acknowledge_error),
                        },
                        last_seq: record.session.last_seq,
                    };
                }
                record.session.last_seq = events
                    .last()
                    .map_or(record.session.last_seq, |event| event.seq);
            }
            let last_seq = record.session.last_seq;
            after_allocation(last_seq);
            let change = if matches!(mode, CommitMode::UpdateInheriting) {
                state
                    .local_tail
                    .clone()
                    .or_else(|| state.durable.clone())
                    .map(|before| {
                        let fields = owned_fields
                            .unwrap_or_else(|| RecordFields::for_commit(&before, &record, &events));
                        let change = Box::new(RecordChange { before, fields });
                        state.local_tail = Some(Box::new(change.next_local_tail(&record)));
                        change
                    })
            } else {
                None
            };
            if matches!(mode, CommitMode::UpdateInheriting) && change.is_none() {
                state.local_tail = Some(Box::new(record.clone()));
            }
            state.queued.push_back(PendingOperation::Commit {
                write_id: format!("sesw_{}", uuid::Uuid::new_v4().simple()),
                mode,
                record: Box::new(record),
                change,
                events,
                complete,
                observed,
                retained_outcome,
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
            complete: CommitReceiver {
                complete: receiver,
                acknowledge_error: Some(acknowledge_error),
            },
            last_seq,
        }
    }

    fn enqueue_delete(self: &Arc<Self>) -> oneshot::Receiver<Result<(), StoreError>> {
        let (complete, receiver) = oneshot::channel();
        self.enqueue(PendingOperation::Delete { complete });
        receiver
    }

    /// Wait behind every operation already in this lane.
    pub(crate) async fn fence(self: &Arc<Self>) {
        let (complete, receiver) = oneshot::channel();
        self.enqueue(PendingOperation::Fence { complete });
        let _ = receiver.await;
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
                    change,
                    mut events,
                    complete,
                    observed,
                    retained_outcome,
                } => {
                    let mut reporter = CommitReporter {
                        complete: Some(complete),
                        observed: Some(observed),
                        retain_after_cancellation: !matches!(mode, CommitMode::Create),
                        retained_outcome,
                    };
                    let result = self
                        .commit(
                            &write_id,
                            mode,
                            &mut record,
                            change.as_deref(),
                            &mut events,
                            &mut reporter,
                        )
                        .await
                        .map(|()| *record);
                    if result.is_err() {
                        if !events.is_empty() {
                            self.abandon_sequence_reservation(events.len());
                        }
                        if matches!(mode, CommitMode::UpdateInheriting) {
                            self.rebuild_local_lineage();
                        }
                    }
                    if let Some(retained_outcome) = reporter.retained_outcome.take() {
                        let _ = retained_outcome.send(result.clone());
                    }
                    if let Some(complete) = reporter.complete.take() {
                        if let Err(Err(error)) = complete.send(result.map(CommitDelivery::plain)) {
                            tracing::error!(
                                session_id = %self.id,
                                error = %error,
                                "a detached durable session write failed"
                            );
                        }
                    }
                }
                PendingOperation::Delete { complete } => {
                    let result = self.delete().await;
                    let _ = complete.send(result);
                }
                PendingOperation::Fence { complete } => {
                    let _ = complete.send(());
                }
                PendingOperation::ObserveRecord(record) => {
                    self.remember_durable(*record);
                }
                #[cfg(test)]
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
        change: Option<&RecordChange>,
        events: &mut [Event],
        reporter: &mut CommitReporter,
    ) -> Result<(), StoreError> {
        if matches!(mode, CommitMode::UpdateInheriting) {
            self.prepare_inheriting_record(record, change)?;
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
                    self.remember_durable(record.clone());
                    return Ok(());
                }
                Err(StoreError::SequenceTaken { expected, .. }) if !events.is_empty() => {
                    if self.durably_terminated().await && !only_terminal_detach(events) {
                        return Err(StoreError::SessionTerminated {
                            id: self.id.clone(),
                        });
                    }
                    self.resequence_from(record, events, expected)?;
                    tracing::warn!(
                        session_id = %self.id,
                        attempt,
                        from_seq = expected,
                        "a replica advanced the durable event tail; rebasing this write unit"
                    );
                }
                Err(StoreError::NonContiguousAppend { expected, .. }) if !events.is_empty() => {
                    if self.durably_terminated().await && !only_terminal_detach(events) {
                        return Err(StoreError::SessionTerminated {
                            id: self.id.clone(),
                        });
                    }
                    self.resequence_from(record, events, expected)?;
                    tracing::warn!(
                        session_id = %self.id,
                        attempt,
                        from_seq = expected,
                        "an earlier local write stopped; closing its uncommitted sequence gap"
                    );
                }
                Err(StoreError::RevisionConflict { .. })
                    if (!events.is_empty() || change.is_some())
                        && !matches!(mode, CommitMode::Create) =>
                {
                    match self.rebase_conflicted_write(record, change, events).await {
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
                            self.retry_or_stop(error, unavailable_attempt, reporter)
                                .await?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error @ StoreError::Unavailable(_)) => {
                    unavailable_attempt = unavailable_attempt.saturating_add(1);
                    self.retry_or_stop(error, unavailable_attempt, reporter)
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Build an inheriting write from the authoritative record, never merely
    /// from an observed revision number.
    fn prepare_inheriting_record(
        &self,
        record: &mut SessionRecord,
        change: Option<&RecordChange>,
    ) -> Result<(), StoreError> {
        let Some(change) = change else {
            // A lane with no predecessor cannot prove inheritance. Retaining
            // the snapshot's own revision makes it an exact CAS; a conflict is
            // safer than authorising a full replacement from a revision alone.
            return Ok(());
        };
        let durable = self
            .durable_record()
            .unwrap_or_else(|| (*change.before).clone());
        *record = change.apply(record, durable)?;
        Ok(())
    }

    fn durable_record(&self) -> Option<SessionRecord> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .durable
            .as_deref()
            .cloned()
    }

    fn remember_durable(&self, record: SessionRecord) {
        self.revision.fetch_max(record.revision, Ordering::AcqRel);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .durable
            .as_ref()
            .is_none_or(|known| record.revision >= known.revision)
        {
            state.durable = Some(Box::new(record));
        }
    }

    /// Rebase the current unit and every queued reservation as one FIFO.
    ///
    /// The store supplies the authoritative first position. Holding the lane
    /// lock while all later reservations move with it prevents a new producer
    /// from allocating against the half-rebased queue.
    fn resequence_from(
        &self,
        record: &mut SessionRecord,
        events: &mut [Event],
        first: u64,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let queued_events = state.queued.iter().try_fold(0_u64, |total, operation| {
            let count = match operation {
                PendingOperation::Commit { events, .. } => u64::try_from(events.len())
                    .map_err(|_| StoreError::Malformed("event batch is too large".into()))?,
                PendingOperation::Delete { .. }
                | PendingOperation::Fence { .. }
                | PendingOperation::ObserveRecord(_) => 0,
                #[cfg(test)]
                PendingOperation::ObserveRevision(_) => 0,
            };
            total.checked_add(count).ok_or_else(|| {
                StoreError::Malformed(format!("event sequence overflow for {}", self.id))
            })
        })?;
        let current_count = u64::try_from(events.len())
            .map_err(|_| StoreError::Malformed("event batch is too large".into()))?;
        let final_tail = first
            .checked_sub(1)
            .and_then(|tail| tail.checked_add(current_count))
            .and_then(|tail| tail.checked_add(queued_events))
            .ok_or_else(|| {
                StoreError::Malformed(format!("event sequence overflow for {}", self.id))
            })?;

        renumber(&self.id, events, first)?;
        let mut tail = events.last().map_or(first - 1, |event| event.seq);
        record.session.last_seq = tail;
        for operation in &mut state.queued {
            let PendingOperation::Commit { record, events, .. } = operation else {
                continue;
            };
            if events.is_empty() {
                record.session.last_seq = tail;
                continue;
            }
            let next = tail.checked_add(1).ok_or_else(|| {
                StoreError::Malformed(format!("event sequence overflow for {}", self.id))
            })?;
            renumber(&self.id, events, next)?;
            tail = events.last().map_or(tail, |event| event.seq);
            record.session.last_seq = tail;
        }
        debug_assert_eq!(tail, final_tail);
        self.sequences.reconcile(&self.id, final_tail);
        if let Some(local_tail) = state.local_tail.as_mut() {
            local_tail.session.last_seq = final_tail;
        }
        Ok(())
    }

    /// Remove one failed unit's reservation and shift every successor with it.
    fn abandon_sequence_reservation(&self, count: usize) {
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        for operation in &mut state.queued {
            let PendingOperation::Commit {
                mode,
                record,
                events,
                ..
            } = operation
            else {
                continue;
            };
            for event in events.iter_mut() {
                event.seq = event.seq.saturating_sub(count);
            }
            if !events.is_empty() || matches!(mode, CommitMode::UpdateInheriting) {
                record.session.last_seq = record.session.last_seq.saturating_sub(count);
            }
        }
        self.sequences.rewind(&self.id, count);
        if let (Some(local_tail), Some(tail)) =
            (state.local_tail.as_mut(), self.sequences.current(&self.id))
        {
            local_tail.session.last_seq = tail;
        }
    }

    /// Remove a failed inheriting proposal from every queued predecessor.
    ///
    /// `local_tail` advances when work is enqueued so successors can describe
    /// their changes without waiting for I/O. If one of those writes is
    /// explicitly rejected, retaining its intended record would make later
    /// proposals conflict with state that never existed. Rebuild the queued
    /// lineage from the latest durable record while holding the same lane lock
    /// used for enqueueing.
    fn rebuild_local_lineage(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut local_tail = state.durable.clone();
        for operation in &mut state.queued {
            match operation {
                PendingOperation::Commit {
                    mode: CommitMode::UpdateInheriting,
                    record,
                    change,
                    events,
                    ..
                } => {
                    let Some(before) = local_tail.take() else {
                        *change = None;
                        local_tail = Some(record.clone());
                        continue;
                    };
                    let fields = change.as_deref().map_or_else(
                        || RecordFields::for_commit(&before, record, events),
                        |change| change.fields,
                    );
                    let rebuilt = Box::new(RecordChange { before, fields });
                    local_tail = Some(Box::new(rebuilt.next_local_tail(record)));
                    *change = Some(rebuilt);
                }
                PendingOperation::ObserveRecord(record) => {
                    local_tail = Some(record.clone());
                }
                PendingOperation::Commit { .. }
                | PendingOperation::Delete { .. }
                | PendingOperation::Fence { .. } => {}
                #[cfg(test)]
                PendingOperation::ObserveRevision(_) => {}
            }
        }
        state.local_tail = local_tail;
    }

    async fn rebase_conflicted_write(
        &self,
        record: &mut SessionRecord,
        change: Option<&RecordChange>,
        events: &mut [Event],
    ) -> Result<(), StoreError> {
        let durable =
            self.store
                .get(&self.id)
                .await?
                .ok_or_else(|| StoreError::SessionDeleted {
                    id: self.id.clone(),
                })?;
        if durable.session.status.is_terminal() && !only_terminal_detach(events) {
            return Err(StoreError::SessionTerminated {
                id: self.id.clone(),
            });
        }
        self.remember_durable(durable.clone());

        let intended = record.clone();
        *record = match change {
            Some(change) => change.apply(&intended, durable)?,
            None => rebase_semantic_record(&intended, durable, events)?,
        };
        let first = record.session.last_seq.checked_add(1).ok_or_else(|| {
            StoreError::Malformed(format!("event sequence overflow for {}", self.id))
        })?;
        self.resequence_from(record, events, first)?;
        Ok(())
    }

    async fn retry_or_stop(
        &self,
        error: StoreError,
        unavailable_attempt: usize,
        reporter: &mut CommitReporter,
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
            if let Some(complete) = reporter.complete.take() {
                if complete.send(Err(error.clone())).is_ok() {
                    #[cfg(test)]
                    self.failure_delivered.notify_one();
                    if reporter
                        .observed
                        .take()
                        .expect("every commit reporter has an observation channel")
                        .await
                        .is_ok()
                    {
                        // The receiver acknowledges in the same poll that
                        // observes the error. Only then can the worker give up
                        // ownership without a cancellation window.
                        return Err(error);
                    }
                }
            }
            if !reporter.retain_after_cancellation {
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
                Ok(()) => {
                    let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                    state.durable = None;
                    state.local_tail = None;
                    return Ok(());
                }
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
    pub(crate) fn queued(&self) -> usize {
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

    /// Persist an exact proposal whose required side effects outlive its waiter.
    pub(crate) fn persist_durably_with<T, H, F, Fut, R, RFut, U, UFut>(
        &self,
        record: SessionRecord,
        events: Vec<Event>,
        continuation: F,
        on_rejected: R,
        on_undelivered: U,
    ) -> CommitReceiver<T>
    where
        T: Send + 'static,
        H: Send + 'static,
        F: FnOnce(SessionRecord) -> Fut + Send + 'static,
        Fut: Future<Output = (T, H)> + Send + 'static,
        R: FnOnce(StoreError) -> RFut + Send + 'static,
        RFut: Future<Output = ()> + Send + 'static,
        U: FnOnce(T, H) -> UFut + Send + 'static,
        UFut: Future<Output = ()> + Send + 'static,
    {
        let id = record.session.id.clone();
        let lane = self.event_logs.lane(&id);
        let (enqueued, retained_outcome) =
            lane.enqueue_retained_commit(CommitMode::UpdateExact, record, events, None, |_| {});
        retain_commit(
            id,
            enqueued.complete,
            retained_outcome,
            continuation,
            on_rejected,
            on_undelivered,
        )
    }

    /// Delete a session and its history behind every local write already queued.
    pub(crate) async fn delete_durably(&self, id: &SessionId) -> Result<(), StoreError> {
        self.event_logs
            .lane(id)
            .enqueue_delete()
            .await
            .map_err(|_| StoreError::Unavailable("session delete worker stopped".into()))?
    }

    /// Repair one still-current live entry after its retained proposal is rejected.
    ///
    /// The fence lets every successor that was already behind the rejected unit
    /// finish any resequencing before its lane tail is copied back. The screen
    /// identity prevents a delayed repair from touching a replacement entry that
    /// happens to use the same session id.
    pub(crate) async fn synchronize_live_tail_for(
        &self,
        id: &SessionId,
        screen: &Arc<tokio::sync::Mutex<Box<dyn crate::terminal::ScreenModel>>>,
    ) {
        let event_log = {
            let entries = self.entries.read().await;
            entries.get(id).and_then(|entry| {
                Arc::ptr_eq(&entry.screen, screen).then(|| Arc::clone(&entry.event_log))
            })
        };
        let Some(event_log) = event_log else {
            return;
        };
        event_log.fence().await;

        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(id) else {
            return;
        };
        if Arc::ptr_eq(&entry.screen, screen) {
            event_log.synchronize_tail(&mut entry.session.last_seq);
        }
    }

    /// Wait for every earlier event snapshot and synchronize the live event tail.
    ///
    /// The empty commit is an ordering and cursor barrier only. Semantic state is
    /// owned by the event-bearing commit that describes it; replaying differences
    /// from this later live snapshot could resurrect a semantic write the store
    /// rejected.
    pub(crate) async fn flush(&self, id: &SessionId) {
        let receiver = {
            let entries = self.entries.read().await;
            let Some(entry) = entries.get(id) else {
                return;
            };
            // Queue before releasing the entry snapshot: a later writer cannot
            // enqueue newer state ahead of this cursor barrier.
            entry
                .event_log
                .enqueue_commit(
                    CommitMode::UpdateInheriting,
                    record_from_entry(entry),
                    Vec::new(),
                )
                .complete
        };
        match receiver
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))
            .and_then(|result| result)
        {
            Ok(record) => {
                if let Some(entry) = self.entries.write().await.get_mut(id) {
                    let event_log = Arc::clone(&entry.event_log);
                    entry.session.last_seq = record.session.last_seq;
                    event_log.synchronize_tail(&mut entry.session.last_seq);
                }
            }
            Err(error) => {
                tracing::error!(session_id = %id, error = %error, "cannot persist session");
            }
        }
    }
}

fn record_from_entry(entry: &SessionEntry) -> SessionRecord {
    let mut session = entry.session.clone();
    session.last_seq = entry.event_log.known_tail(session.last_seq);
    let mut record = SessionRecord::new(session);
    record.runtime_id = entry.runtime.as_ref().map(|handle| handle.0.clone());
    record.context_ref = entry.context_ref.clone();
    record.revision = entry.event_log.revision.load(Ordering::Acquire);
    record
}

/// Queue a record-only transcript reference update with explicit narrow ownership.
///
/// Empty commits normally own no semantic fields because their snapshots may
/// include rejected live state. Transcript capture is the exception: it has an
/// awaited operation whose only intended change is the context reference.
pub(crate) fn update_context_reference(entry: &SessionEntry, reference: String) -> CommitReceiver {
    let mut record = record_from_entry(entry);
    record.context_ref = Some(reference);
    entry
        .event_log
        .enqueue_commit_with(
            CommitMode::UpdateInheriting,
            record,
            Vec::new(),
            Some(RecordFields {
                context: true,
                ..RecordFields::default()
            }),
            |_| {},
        )
        .complete
}

impl RecordFields {
    fn for_commit(before: &SessionRecord, intended: &SessionRecord, events: &[Event]) -> Self {
        if events.is_empty() {
            // An empty unit is only an ordering/cursor barrier. Its live snapshot
            // may still contain semantic state from an earlier rejected event,
            // so differences in that snapshot can never establish ownership.
            return Self::default();
        }

        let mut fields = Self::for_events(events);

        fields.configuration = before.session.owner != intended.session.owner
            || before.session.kernel != intended.session.kernel
            || before.session.environment_profile != intended.session.environment_profile
            || before.session.granted_services != intended.session.granted_services
            || before.session.granted_skills != intended.session.granted_skills
            || before.session.pricing != intended.session.pricing
            || before.session.created_at != intended.session.created_at;
        fields.runtime = before.runtime_id != intended.runtime_id;
        fields.context = before.context_ref != intended.context_ref;
        fields
    }

    fn for_events(events: &[Event]) -> Self {
        let mut fields = Self::default();
        for event in events {
            match event.kind {
                EventKind::StatusChanged { .. } => fields.status = true,
                EventKind::TerminalResized { .. } | EventKind::TerminalAttached => {
                    fields.geometry = true
                }
                EventKind::PlaneChanged { .. } => fields.plane = true,
                EventKind::RuntimeLost { .. } => fields.runtime_loss = true,
                _ => {}
            }
        }
        fields
    }
}

impl RecordChange {
    /// Advance the raw local lineage only through fields this operation owns.
    ///
    /// A workflow may mutate status and then append an owner message and turn
    /// boundary before its `StatusChanged` event. Keeping those earlier events
    /// fieldless makes the status delta belong to the event that describes it.
    fn next_local_tail(&self, intended: &SessionRecord) -> SessionRecord {
        let mut next = (*self.before).clone();
        self.copy_selected(intended, &mut next);
        next.session.last_seq = intended.session.last_seq;
        next.revision = intended.revision;
        next
    }

    /// Apply only this operation's semantic fields to a fresh durable snapshot.
    fn apply(
        &self,
        intended: &SessionRecord,
        mut durable: SessionRecord,
    ) -> Result<SessionRecord, StoreError> {
        let before = &*self.before;
        if before.session.id != intended.session.id || durable.session.id != intended.session.id {
            return Err(StoreError::Malformed(
                "one record change named more than one session".into(),
            ));
        }

        let expected = before.revision;
        let actual = durable.revision;
        let configuration = self.fields.configuration;
        let status = self.fields.status;
        let geometry = self.fields.geometry;
        let plane = self.fields.plane;
        let recovery = self.fields.status || self.fields.runtime_loss;
        let runtime = self.fields.runtime
            || self.fields.plane
            || self.fields.runtime_loss
            || (self.fields.status && !intended.session.status.wants_runtime());
        let mut compatible = true;

        compatible &= rebase_selected(
            plane,
            &mut durable.session.options,
            &before.session.options,
            &intended.session.options,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.owner,
            &before.session.owner,
            &intended.session.owner,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.kernel,
            &before.session.kernel,
            &intended.session.kernel,
        );
        compatible &= rebase_selected(
            status,
            &mut durable.session.status,
            &before.session.status,
            &intended.session.status,
        );
        compatible &= rebase_selected(
            recovery,
            &mut durable.session.recovery_state,
            &before.session.recovery_state,
            &intended.session.recovery_state,
        );
        compatible &= rebase_selected(
            status,
            &mut durable.session.stop_reason,
            &before.session.stop_reason,
            &intended.session.stop_reason,
        );
        compatible &= rebase_selected(
            geometry,
            &mut durable.session.geometry,
            &before.session.geometry,
            &intended.session.geometry,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.environment_profile,
            &before.session.environment_profile,
            &intended.session.environment_profile,
        );
        compatible &= rebase_selected(
            plane,
            &mut durable.session.runtime_version,
            &before.session.runtime_version,
            &intended.session.runtime_version,
        );
        compatible &= rebase_selected(
            plane,
            &mut durable.session.rotations,
            &before.session.rotations,
            &intended.session.rotations,
        );
        compatible &= rebase_selected(
            plane,
            &mut durable.session.rotated_at,
            &before.session.rotated_at,
            &intended.session.rotated_at,
        );
        compatible &= rebase_selected(
            plane,
            &mut durable.session.capabilities,
            &before.session.capabilities,
            &intended.session.capabilities,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.granted_services,
            &before.session.granted_services,
            &intended.session.granted_services,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.granted_skills,
            &before.session.granted_skills,
            &intended.session.granted_skills,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.pricing,
            &before.session.pricing,
            &intended.session.pricing,
        );
        compatible &= rebase_selected(
            configuration,
            &mut durable.session.created_at,
            &before.session.created_at,
            &intended.session.created_at,
        );
        compatible &= rebase_selected(
            runtime,
            &mut durable.runtime_id,
            &before.runtime_id,
            &intended.runtime_id,
        );
        compatible &= rebase_selected(
            self.fields.context,
            &mut durable.context_ref,
            &before.context_ref,
            &intended.context_ref,
        );

        if !compatible {
            return Err(StoreError::RevisionConflict { expected, actual });
        }
        // `last_seq` belongs to the reservation FIFO and `revision` belongs to
        // the store. Both stay on the authoritative record and are updated by
        // the commit path, never copied from a queued snapshot.
        Ok(durable)
    }

    fn copy_selected(&self, intended: &SessionRecord, target: &mut SessionRecord) {
        let configuration = self.fields.configuration;
        let status = self.fields.status;
        let geometry = self.fields.geometry;
        let plane = self.fields.plane;
        let recovery = self.fields.status || self.fields.runtime_loss;
        let runtime = self.fields.runtime
            || self.fields.plane
            || self.fields.runtime_loss
            || (self.fields.status && !intended.session.status.wants_runtime());

        copy_selected(
            plane,
            &mut target.session.options,
            &intended.session.options,
        );
        copy_selected(
            configuration,
            &mut target.session.owner,
            &intended.session.owner,
        );
        copy_selected(
            configuration,
            &mut target.session.kernel,
            &intended.session.kernel,
        );
        copy_selected(status, &mut target.session.status, &intended.session.status);
        copy_selected(
            recovery,
            &mut target.session.recovery_state,
            &intended.session.recovery_state,
        );
        copy_selected(
            status,
            &mut target.session.stop_reason,
            &intended.session.stop_reason,
        );
        copy_selected(
            geometry,
            &mut target.session.geometry,
            &intended.session.geometry,
        );
        copy_selected(
            configuration,
            &mut target.session.environment_profile,
            &intended.session.environment_profile,
        );
        copy_selected(
            plane,
            &mut target.session.runtime_version,
            &intended.session.runtime_version,
        );
        copy_selected(
            plane,
            &mut target.session.rotations,
            &intended.session.rotations,
        );
        copy_selected(
            plane,
            &mut target.session.rotated_at,
            &intended.session.rotated_at,
        );
        copy_selected(
            plane,
            &mut target.session.capabilities,
            &intended.session.capabilities,
        );
        copy_selected(
            configuration,
            &mut target.session.granted_services,
            &intended.session.granted_services,
        );
        copy_selected(
            configuration,
            &mut target.session.granted_skills,
            &intended.session.granted_skills,
        );
        copy_selected(
            configuration,
            &mut target.session.pricing,
            &intended.session.pricing,
        );
        copy_selected(
            configuration,
            &mut target.session.created_at,
            &intended.session.created_at,
        );
        copy_selected(runtime, &mut target.runtime_id, &intended.runtime_id);
        copy_selected(
            self.fields.context,
            &mut target.context_ref,
            &intended.context_ref,
        );
    }
}

fn rebase_selected<T: Clone + PartialEq>(
    selected: bool,
    durable: &mut T,
    before: &T,
    intended: &T,
) -> bool {
    !selected || rebase_field(durable, before, intended)
}

fn copy_selected<T: Clone>(selected: bool, target: &mut T, intended: &T) {
    if selected {
        target.clone_from(intended);
    }
}

fn rebase_field<T: Clone + PartialEq>(durable: &mut T, before: &T, intended: &T) -> bool {
    if before == intended {
        return true;
    }
    if durable != before && durable != intended {
        return false;
    }
    durable.clone_from(intended);
    true
}

/// Reapply an event unit to a fresh durable snapshot when its semantic
/// precondition still holds.
///
/// History-only writes preserve every field changed by the competing replica.
/// Runtime loss names the exact runtime it observed, so that identity is a
/// sufficient compare precondition. Other record-mutating events do not carry
/// their complete prior state and must return the revision conflict instead of
/// guessing that a newer lifecycle or plane decision is safe to overwrite.
fn rebase_semantic_record(
    intended: &SessionRecord,
    mut durable: SessionRecord,
    events: &[Event],
) -> Result<SessionRecord, StoreError> {
    for event in events {
        match &event.kind {
            EventKind::RuntimeLost { runtime_id, .. }
                if durable.runtime_id.as_deref() == Some(runtime_id.as_str()) =>
            {
                durable.runtime_id = intended.runtime_id.clone();
                durable.session.recovery_state = intended.session.recovery_state;
            }
            EventKind::RuntimeLost { .. }
            | EventKind::StatusChanged { .. }
            | EventKind::TerminalResized { .. }
            | EventKind::PlaneChanged { .. } => {
                return Err(StoreError::RevisionConflict {
                    expected: intended.revision,
                    actual: durable.revision,
                });
            }
            _ => {}
        }
    }
    Ok(durable)
}

fn only_terminal_detach(events: &[Event]) -> bool {
    !events.is_empty()
        && events
            .iter()
            .all(|event| matches!(event.kind, EventKind::TerminalDetached))
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
    // Fire-and-forget events describe state that is already live, so their
    // allocated cursor is live immediately as well.
    entry.session.last_seq = enqueued.last_seq;
    drop(enqueued.complete);
}

/// Queue a semantic event whose intended record is not live state until commit.
///
/// Public operations that act on a runtime use this form so a cross-replica
/// conflict can be reported before the runtime is changed. The ordinary
/// [`append`] path has already observed its state change and therefore leaves
/// the detached lane responsible for persistence after the caller moves on.
pub(crate) fn append_with<T, H, F, Fut, R, RFut, U, UFut>(
    entry: &mut SessionEntry,
    kind: EventKind,
    change: impl FnOnce(&mut SessionRecord),
    continuation: F,
    on_rejected: R,
    on_undelivered: U,
) -> CommitReceiver<T>
where
    T: Send + 'static,
    H: Send + 'static,
    F: FnOnce(SessionRecord) -> Fut + Send + 'static,
    Fut: Future<Output = (T, H)> + Send + 'static,
    R: FnOnce(StoreError) -> RFut + Send + 'static,
    RFut: Future<Output = ()> + Send + 'static,
    U: FnOnce(T, H) -> UFut + Send + 'static,
    UFut: Future<Output = ()> + Send + 'static,
{
    let event = Event {
        session_id: entry.session.id.clone(),
        seq: 0,
        kind,
        appended_at: String::new(),
    };
    let mut record = record_from_entry(entry);
    change(&mut record);
    let id = entry.session.id.clone();
    let event_log = Arc::clone(&entry.event_log);
    let mut live_tail = None;
    let (enqueued, retained_outcome) = event_log.enqueue_retained_commit(
        CommitMode::UpdateInheriting,
        record,
        vec![event],
        None,
        |last_seq| live_tail = Some(last_seq.saturating_sub(1)),
    );
    // A proposal is not live until its continuation installs the committed
    // record. Publish only the lane tail that preceded its reservation.
    if let Some(live_tail) = live_tail {
        entry.session.last_seq = live_tail;
    }
    retain_commit(
        id,
        enqueued.complete,
        retained_outcome,
        continuation,
        on_rejected,
        on_undelivered,
    )
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;

#[cfg(test)]
#[path = "flush_tests.rs"]
mod flush_tests;
