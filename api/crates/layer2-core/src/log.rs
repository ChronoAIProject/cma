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
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::sequence::Sequences;
use crate::service::{SessionEntry, SessionService};
use crate::store::{SessionRecord, SessionStore, StoreError};
use crate::ConversationWriteAuthority;

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

type CommitCompletion = oneshot::Sender<Result<CommitDelivery<SessionRecord>, StoreError>>;
type CommitCompletionSlot = Option<CommitCompletion>;

enum CommitTarget {
    Ordinary {
        complete: CommitCompletionSlot,
        observed: Option<oneshot::Receiver<()>>,
    },
    Retained(Box<dyn RetainedCommitWork>),
}

enum PendingOperation {
    Commit {
        write_id: String,
        mode: CommitMode,
        record: Box<SessionRecord>,
        change: Option<Box<RecordChange>>,
        events: Vec<Event>,
        conversation_authority: Option<ConversationWriteAuthority>,
        target: Option<CommitTarget>,
        progress: CommitProgress,
        /// Set before post-commit work is first polled, so that phase resumes
        /// without replaying a write whose outcome is already known.
        outcome: Option<Box<Result<SessionRecord, StoreError>>>,
    },
    RestartableCommit {
        write_id: String,
        record: Box<SessionRecord>,
        events: Vec<Event>,
        conversation_authority: Option<ConversationWriteAuthority>,
        outcome: Option<oneshot::Sender<Result<SessionRecord, StoreError>>>,
        progress: CommitProgress,
    },
    Delete {
        complete: Option<oneshot::Sender<Result<(), StoreError>>>,
        progress: DeleteProgress,
    },
    Fence {
        complete: Option<oneshot::Sender<()>>,
    },
    RetainedCompletion {
        retained: Box<dyn RetainedCommitWork>,
        outcome: Box<Result<SessionRecord, StoreError>>,
    },
    ObserveRecord(Option<Box<SessionRecord>>),
    #[cfg(test)]
    ObserveRevision(u64),
}

#[derive(Default)]
struct CommitProgress {
    prepared: bool,
    attempt: usize,
    unavailable_attempt: usize,
    reported_error: Option<StoreError>,
}

#[derive(Default)]
struct DeleteProgress {
    /// Only completed unavailable results consume the caller's report boundary.
    ///
    /// Deletes have no semantic continuation to retain after their waiter goes
    /// away. They therefore finish the lane after four completed unavailable
    /// results even when delivery fails, rather than retrying forever for a
    /// caller that can no longer observe the result.
    unavailable_attempt: usize,
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
    /// A handover token this change must still own after any rebase.
    ///
    /// Kept separately from `before`: rejection repair rebuilds that snapshot
    /// from the newest durable record, but doing so must never let an obsolete
    /// finalizer or rollback inherit a newer handover's token.
    rotation_precondition: Option<RotationPrecondition>,
    /// Whether this exact lifecycle repair may inherit a terminal row.
    ///
    /// Ordinary writes remain fenced once termination is durable. Compatibility
    /// termination is the sole exception: it may need to clear an already-terminal
    /// handover descriptor after skill cleanup advanced an unrelated revision.
    /// The rotation precondition still fences the exact lifecycle/runtime boundary.
    terminal_lifecycle_repair: bool,
}

#[derive(Clone)]
struct RotationPrecondition {
    claim: Option<String>,
    status: crate::session::SessionStatus,
    /// Exact published runtime, when finalization happens after a switch.
    runtime_id: Option<Option<String>>,
    /// Exact compatibility descriptor, when this transition consumes one.
    pending_handover: Option<Option<crate::store::PendingHandover>>,
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
    rotation: bool,
    handover: bool,
    skill_grant: bool,
    skill_packs: bool,
}

#[derive(Default)]
struct CommitSemantics {
    owned_fields: Option<RecordFields>,
    rotation_precondition: Option<RotationPrecondition>,
    terminal_lifecycle_repair: bool,
}

struct CommitRequest {
    mode: CommitMode,
    record: SessionRecord,
    events: Vec<Event>,
    semantics: CommitSemantics,
    conversation_authority: Option<ConversationWriteAuthority>,
}

struct CommitContext<'a> {
    write_id: &'a str,
    mode: CommitMode,
    change: Option<&'a RecordChange>,
    conversation_authority: Option<&'a ConversationWriteAuthority>,
}

struct EnqueuedCommit {
    complete: CommitReceiver,
    last_seq: u64,
}

type RestartSignal = Pin<Box<dyn Future<Output = ()> + Send>>;

fn restart_signal(changed: Arc<tokio::sync::Notify>) -> RestartSignal {
    Box::pin(async move { changed.notified().await })
}

struct LaneReceiver<T> {
    complete: oneshot::Receiver<T>,
    lane: Arc<SessionLog>,
    restart: RestartSignal,
}

impl<T> LaneReceiver<T> {
    fn new(complete: oneshot::Receiver<T>, lane: Arc<SessionLog>) -> Self {
        let restart = restart_signal(Arc::clone(&lane.restart_needed));
        Self {
            complete,
            lane,
            restart,
        }
    }
}

/// A lane result whose waiter can take worker ownership onto another runtime.
///
/// The restart listener is polled before `kick`. A worker cancellation either
/// clears `running` before that check or notifies the already-registered waiter,
/// so enqueueing immediately before cancellation cannot lose the restart.
impl<T> Future for LaneReceiver<T> {
    type Output = Result<T, oneshot::error::RecvError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.restart.as_mut().poll(context).is_ready() {
            self.restart = restart_signal(Arc::clone(&self.lane.restart_needed));
            let _ = self.restart.as_mut().poll(context);
        }
        self.lane.kick();
        Pin::new(&mut self.complete).poll(context)
    }
}

pub(crate) struct RestartableCommit {
    lane: Arc<SessionLog>,
    outcome: LaneReceiver<Result<SessionRecord, StoreError>>,
}

impl RestartableCommit {
    pub(crate) fn restart(&self) {
        self.lane.kick();
    }

    pub(crate) async fn committed(&mut self) -> Result<SessionRecord, StoreError> {
        (&mut self.outcome)
            .await
            .map_err(|_| StoreError::Unavailable("retained cleanup writer stopped".into()))?
    }
}

type DeliveryCleanup<T> = Box<dyn FnOnce(T) + Send>;

/// A successful result that still owns its cancellation cleanup until polled.
///
/// Sending through a oneshot is not observation: the receiver can be canceled
/// after `send` succeeds but before it polls the value. Keeping cleanup in the
/// value itself closes that window. Polling extracts the output and disarms the
/// cleanup; dropping an unread delivery hands it to its cancellation owner
/// synchronously, without assuming the dropping thread has a live runtime.
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

    fn retained<H, U>(output: T, ownership: H, cleanup: U) -> Self
    where
        T: Send + 'static,
        H: Send + 'static,
        U: FnOnce(T, H) + Send + 'static,
    {
        Self {
            output: Some(output),
            cleanup: Some(Box::new(move |output| cleanup(output, ownership))),
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
        cleanup(output);
    }
}

pub(crate) type CleanupAttempt<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// A cancellation cleanup whose owned state survives interruption of one run.
pub(crate) trait DeferredCleanup: Send {
    fn session_id(&self) -> &SessionId;
    fn attempt(&mut self) -> CleanupAttempt<'_>;
}

#[derive(Default)]
struct DeferredCleanups {
    state: StdMutex<DeferredCleanupState>,
    drains: StdMutex<HashMap<SessionId, Weak<CleanupGate>>>,
}

#[derive(Default)]
struct DeferredCleanupState {
    pending: VecDeque<Box<dyn DeferredCleanup>>,
}

#[derive(Default)]
struct CleanupGate {
    lease: tokio::sync::Mutex<()>,
    changed: tokio::sync::Notify,
}

impl DeferredCleanups {
    fn retain(self: &Arc<Self>, cleanup: Box<dyn DeferredCleanup>) {
        let id = cleanup.session_id().clone();
        let gate = self.drain_gate(&id);
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .push_back(cleanup);
        gate.changed.notify_waiters();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let cleanups = Arc::clone(self);
            runtime.spawn(async move { cleanups.drain_at(&id, &gate).await });
        }
    }

    async fn drain_at(self: &Arc<Self>, id: &SessionId, gate: &CleanupGate) {
        let lease = gate.lease.lock().await;
        CleanupDrain {
            owner: Arc::clone(self),
            gate,
            id: id.clone(),
            cleanup: None,
            _lease: lease,
        }
        .finish()
        .await;
    }

    fn drain_gate(&self, id: &SessionId) -> Arc<CleanupGate> {
        let mut drains = self
            .drains
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        drains.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = drains.get(id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(CleanupGate::default());
        drains.insert(id.clone(), Arc::downgrade(&gate));
        gate
    }

    fn has_pending(&self, id: &SessionId) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .iter()
            .any(|cleanup| cleanup.session_id() == id)
    }

    #[cfg(test)]
    fn drain_users(&self, id: &SessionId) -> usize {
        self.drains
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .map_or(0, Weak::strong_count)
    }
}

/// Serializes one session's drain and returns its current job before waking a waiter.
struct CleanupDrain<'a> {
    owner: Arc<DeferredCleanups>,
    gate: &'a CleanupGate,
    id: SessionId,
    cleanup: Option<Box<dyn DeferredCleanup>>,
    _lease: tokio::sync::MutexGuard<'a, ()>,
}

impl CleanupDrain<'_> {
    async fn finish(mut self) {
        loop {
            let cleanup = {
                let mut state = self
                    .owner
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state
                    .pending
                    .iter()
                    .position(|cleanup| cleanup.session_id() == &self.id)
                    .and_then(|position| state.pending.remove(position))
            };
            let Some(cleanup) = cleanup else {
                return;
            };
            self.cleanup = Some(cleanup);
            self.cleanup
                .as_mut()
                .expect("a retained cleanup runs at most once per attempt")
                .attempt()
                .await;
            self.cleanup.take();
        }
    }
}

impl Drop for CleanupDrain<'_> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            self.owner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pending
                .push_front(cleanup);
            self.gate.changed.notify_waiters();
        }
        // `_lease` is a field, so Rust drops it only after this method returns.
        // A woken waiter therefore always observes the interrupted job queued.
    }
}

/// Reports that an explicit failure was actually observed in the caller's
/// poll, rather than merely placed in a channel it canceled without reading.
pub(crate) struct CommitReceiver<T = SessionRecord> {
    complete: oneshot::Receiver<Result<CommitDelivery<T>, StoreError>>,
    acknowledge_error: Option<oneshot::Sender<()>>,
    lane: Arc<SessionLog>,
    restart: RestartSignal,
}

impl<T> Future for CommitReceiver<T> {
    type Output = Result<Result<T, StoreError>, oneshot::error::RecvError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.restart.as_mut().poll(context).is_ready() {
            self.restart = restart_signal(Arc::clone(&self.lane.restart_needed));
            let _ = self.restart.as_mut().poll(context);
        }
        self.lane.kick();
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
    fn new(
        complete: oneshot::Receiver<Result<CommitDelivery<T>, StoreError>>,
        acknowledge_error: Option<oneshot::Sender<()>>,
        lane: Arc<SessionLog>,
    ) -> Self {
        let restart = restart_signal(Arc::clone(&lane.restart_needed));
        Self {
            complete,
            acknowledge_error,
            lane,
            restart,
        }
    }

    pub(crate) async fn committed(self) -> Result<T, StoreError> {
        self.await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))?
    }
}

type RetainedAttempt<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type RetainedContinuation<T, H> =
    Box<dyn FnOnce(SessionRecord) -> RetainedAttempt<'static, (T, H)> + Send>;
type RetainedRejection = Box<dyn FnOnce(StoreError) -> RetainedAttempt<'static, ()> + Send>;
type RetainedUndelivered<T, H> = Box<dyn FnOnce(T, H) + Send>;

fn retained_executor() -> &'static tokio::runtime::Handle {
    static EXECUTOR: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    EXECUTOR.get_or_init(|| {
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let _thread = std::thread::Builder::new()
            .name("cma-retained-work".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("the retained-work runtime builds");
                ready
                    .send(runtime.handle().clone())
                    .expect("the retained-work executor has an initializer");
                runtime.block_on(std::future::pending::<()>());
            })
            .expect("the retained-work executor thread starts");
        started
            .recv()
            .expect("the retained-work executor reports readiness")
    })
}

/// Run service-owned cleanup without depending on the dropping thread's runtime.
pub(crate) fn spawn_retained_work(work: impl Future<Output = ()> + Send + 'static) {
    let _detached = retained_executor().spawn(work);
}

/// Type-erased post-commit work kept inside its guarded lane operation.
trait RetainedCommitWork: Send {
    fn report_error(&mut self, error: StoreError) -> bool;
    fn error_observed(&mut self) -> RetainedAttempt<'_, bool>;
    fn clear_error_observation(&mut self);
    fn prepare_outcome(&mut self, outcome: Result<SessionRecord, StoreError>);
    fn finish(&mut self) -> RetainedAttempt<'_, ()>;
}

struct RetainedCommit<T, H> {
    id: SessionId,
    /// Keep the weakly registered lane reachable while this work is queued.
    /// Completion breaks the temporary lane -> work -> lane cycle.
    lane_keepalive: Option<Arc<SessionLog>>,
    report: Option<oneshot::Sender<Result<CommitDelivery<T>, StoreError>>>,
    error_observed: Option<oneshot::Receiver<()>>,
    error_was_observed: bool,
    continuation: Option<RetainedContinuation<T, H>>,
    continuing: Option<oneshot::Receiver<(T, H)>>,
    on_rejected: Option<RetainedRejection>,
    rejecting: Option<oneshot::Receiver<()>>,
    rejection: Option<StoreError>,
    on_undelivered: Option<RetainedUndelivered<T, H>>,
    outcome_prepared: bool,
}

impl<T, H> RetainedCommitWork for RetainedCommit<T, H>
where
    T: Send + 'static,
    H: Send + 'static,
{
    fn report_error(&mut self, error: StoreError) -> bool {
        let Some(report) = self.report.take() else {
            return false;
        };
        if report.send(Err(error)).is_ok() {
            true
        } else {
            self.error_observed.take();
            false
        }
    }

    fn error_observed(&mut self) -> RetainedAttempt<'_, bool> {
        Box::pin(async move {
            let observed = self
                .error_observed
                .as_mut()
                .expect("a reported retained commit keeps its observation channel")
                .await
                .is_ok();
            if observed {
                self.error_was_observed = true;
            }
            observed
        })
    }

    fn clear_error_observation(&mut self) {
        self.error_observed.take();
    }

    fn prepare_outcome(&mut self, outcome: Result<SessionRecord, StoreError>) {
        if self.outcome_prepared {
            return;
        }
        self.outcome_prepared = true;
        match outcome {
            Ok(record) => {
                let continuation = self
                    .continuation
                    .take()
                    .expect("a retained continuation is prepared at most once");
                let (completed, continuing) = oneshot::channel();
                retained_executor().spawn(async move {
                    let output = continuation(record).await;
                    let _ = completed.send(output);
                });
                self.continuing = Some(continuing);
            }
            Err(error) => {
                let on_rejected = self
                    .on_rejected
                    .take()
                    .expect("a retained rejection is prepared at most once");
                let (completed, rejecting) = oneshot::channel();
                let rejection = error.clone();
                retained_executor().spawn(async move {
                    on_rejected(rejection).await;
                    let _ = completed.send(());
                });
                self.rejecting = Some(rejecting);
                self.rejection = Some(error);
            }
        }
    }

    fn finish(&mut self) -> RetainedAttempt<'_, ()> {
        Box::pin(async move {
            if let Some(continuing) = self.continuing.as_mut() {
                let continued = continuing.await;
                self.continuing.take();
                let Ok((output, ownership)) = continued else {
                    let error = StoreError::Unavailable(
                        "retained session continuation task stopped".into(),
                    );
                    if let Some(report) = self.report.take() {
                        let _ = report.send(Err(error.clone()));
                    } else {
                        tracing::error!(
                            session_id = %self.id,
                            %error,
                            "a detached durable session continuation stopped"
                        );
                    }
                    self.on_undelivered.take();
                    self.lane_keepalive.take();
                    return;
                };
                let on_undelivered = self
                    .on_undelivered
                    .take()
                    .expect("a retained success finishes at most once");
                if let Some(report) = self.report.take() {
                    let _ = report.send(Ok(CommitDelivery::retained(
                        output,
                        ownership,
                        on_undelivered,
                    )));
                } else {
                    on_undelivered(output, ownership);
                }
                self.lane_keepalive.take();
                return;
            }

            if let Some(rejecting) = self.rejecting.as_mut() {
                let rejected = rejecting.await;
                self.rejecting.take();
                if rejected.is_err() {
                    let error =
                        StoreError::Unavailable("retained session rejection task stopped".into());
                    if let Some(report) = self.report.take() {
                        let _ = report.send(Err(error.clone()));
                    } else if !self.error_was_observed {
                        tracing::error!(
                            session_id = %self.id,
                            %error,
                            "a detached durable session rejection stopped"
                        );
                    }
                    self.rejection.take();
                    self.lane_keepalive.take();
                    return;
                }
                let error = self
                    .rejection
                    .take()
                    .expect("a retained rejection keeps its error until cleanup finishes");
                if let Some(report) = self.report.take() {
                    let _ = report.send(Err(error));
                } else if !self.error_was_observed {
                    tracing::error!(
                        session_id = %self.id,
                        error = %error,
                        "a detached durable session proposal failed"
                    );
                }
                self.lane_keepalive.take();
                return;
            }

            debug_assert!(
                self.outcome_prepared,
                "retained work finishes only after durability"
            );
            self.lane_keepalive.take();
        })
    }
}

/// Build post-commit work before its lane operation can start.
///
/// Closures remain fields of the guarded operation until durability chooses one.
/// The selected continuation then runs to completion on the stable retained-work
/// executor; the lane stores only its runtime-agnostic result receiver.
fn retain_commit<T, H, F, Fut, R, RFut, U>(
    id: SessionId,
    lane: Arc<SessionLog>,
    continuation: F,
    on_rejected: R,
    on_undelivered: U,
) -> (Box<dyn RetainedCommitWork>, CommitReceiver<T>)
where
    T: Send + 'static,
    H: Send + 'static,
    F: FnOnce(SessionRecord) -> Fut + Send + 'static,
    Fut: Future<Output = (T, H)> + Send + 'static,
    R: FnOnce(StoreError) -> RFut + Send + 'static,
    RFut: Future<Output = ()> + Send + 'static,
    U: FnOnce(T, H) + Send + 'static,
{
    let (report, complete) = oneshot::channel();
    let (acknowledge_error, error_observed) = oneshot::channel();
    let retained = RetainedCommit {
        id,
        lane_keepalive: Some(Arc::clone(&lane)),
        report: Some(report),
        error_observed: Some(error_observed),
        error_was_observed: false,
        continuation: Some(Box::new(move |record| Box::pin(continuation(record)))),
        continuing: None,
        on_rejected: Some(Box::new(move |error| Box::pin(on_rejected(error)))),
        rejecting: None,
        rejection: None,
        on_undelivered: Some(Box::new(on_undelivered)),
        outcome_prepared: false,
    };
    (
        Box::new(retained),
        CommitReceiver::new(complete, Some(acknowledge_error), lane),
    )
}

pub(crate) async fn ignore_rejected(_error: StoreError) {}

pub(crate) fn drop_undelivered<T, H>(_output: T, _ownership: H) {}

struct CommitReporter<'a> {
    progress: &'a mut CommitProgress,
    target: Option<&'a mut CommitTarget>,
    retain_after_cancellation: bool,
}

impl CommitReporter<'_> {
    fn report_error(&mut self, error: StoreError) -> bool {
        match self.target.as_deref_mut() {
            Some(CommitTarget::Ordinary { complete, .. }) => complete
                .take()
                .is_some_and(|complete| complete.send(Err(error)).is_ok()),
            Some(CommitTarget::Retained(retained)) => retained.report_error(error),
            None => false,
        }
    }

    async fn error_observed(&mut self) -> bool {
        match self.target.as_deref_mut() {
            Some(CommitTarget::Ordinary { observed, .. }) => observed
                .as_mut()
                .expect("a reported commit retains its observation channel")
                .await
                .is_ok(),
            Some(CommitTarget::Retained(retained)) => retained.error_observed().await,
            None => unreachable!("a reported commit has a completion target"),
        }
    }

    fn clear_error_observation(&mut self) {
        match self.target.as_deref_mut() {
            Some(CommitTarget::Ordinary { observed, .. }) => {
                observed.take();
            }
            Some(CommitTarget::Retained(retained)) => retained.clear_error_observation(),
            None => {}
        }
    }
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

#[cfg(test)]
struct FinalRejectionPause {
    reached: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

struct RunningOperation {
    lane: Arc<SessionLog>,
    operation: Option<PendingOperation>,
}

impl RunningOperation {
    fn operation_mut(&mut self) -> &mut PendingOperation {
        self.operation
            .as_mut()
            .expect("a running lane operation remains owned until it finishes")
    }

    fn finish(mut self) {
        self.operation
            .take()
            .expect("a lane operation finishes at most once");
    }
}

impl Drop for RunningOperation {
    fn drop(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        let mut state = self
            .lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.queued.push_front(operation);
        state.running = false;
        drop(state);
        self.lane.restart_needed.notify_waiters();
    }
}

/// Makes a queued lane restartable when its spawned worker is never polled.
struct UnpolledLaneWorker {
    lane: Arc<SessionLog>,
    started: bool,
}

impl UnpolledLaneWorker {
    async fn run(mut self) {
        self.started = true;
        let lane = Arc::clone(&self.lane);
        lane.run().await;
    }
}

impl Drop for UnpolledLaneWorker {
    fn drop(&mut self) {
        if self.started {
            return;
        }
        self.lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running = false;
        self.lane.restart_needed.notify_waiters();
    }
}

/// One process's FIFO for a session.
pub(crate) struct SessionLog {
    id: SessionId,
    store: Arc<dyn SessionStore>,
    sequences: Arc<Sequences>,
    revision: AtomicU64,
    state: StdMutex<LaneState>,
    restart_needed: Arc<tokio::sync::Notify>,
    #[cfg(test)]
    failure_delivered: tokio::sync::Notify,
    #[cfg(test)]
    pause_after_pop: AtomicBool,
    #[cfg(test)]
    operation_popped: tokio::sync::Notify,
    #[cfg(test)]
    release_popped_operation: tokio::sync::Notify,
    #[cfg(test)]
    final_rejection_pause: StdMutex<Option<FinalRejectionPause>>,
}

impl SessionLog {
    fn new(id: SessionId, store: Arc<dyn SessionStore>, sequences: Arc<Sequences>) -> Self {
        Self {
            id,
            store,
            sequences,
            revision: AtomicU64::new(0),
            state: StdMutex::new(LaneState::default()),
            restart_needed: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            failure_delivered: tokio::sync::Notify::new(),
            #[cfg(test)]
            pause_after_pop: AtomicBool::new(false),
            #[cfg(test)]
            operation_popped: tokio::sync::Notify::new(),
            #[cfg(test)]
            release_popped_operation: tokio::sync::Notify::new(),
            #[cfg(test)]
            final_rejection_pause: StdMutex::new(None),
        }
    }

    #[cfg(test)]
    fn pause_next_operation_after_pop(&self) {
        assert!(!self.pause_after_pop.swap(true, Ordering::SeqCst));
    }

    #[cfg(test)]
    fn pause_next_final_rejection(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (reached, wait_until_reached) = std::sync::mpsc::channel();
        let (release, wait_until_released) = std::sync::mpsc::channel();
        let mut pause = self
            .final_rejection_pause
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(pause.is_none(), "only one final rejection can be paused");
        *pause = Some(FinalRejectionPause {
            reached,
            release: wait_until_released,
        });
        (wait_until_reached, release)
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
                .push_back(PendingOperation::ObserveRecord(Some(Box::new(
                    record.clone(),
                ))));
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };
        if start {
            self.spawn_worker();
        }
    }

    /// Seed the local allocator from a durable record before exposing a live
    /// entry backed by this lane.
    pub(crate) fn observe_tail(&self, tail: u64) {
        let _state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        self.sequences.observe_durable(&self.id, tail);
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

    fn enqueue_conversation_commit(
        self: &Arc<Self>,
        mode: CommitMode,
        record: SessionRecord,
        events: Vec<Event>,
        authority: ConversationWriteAuthority,
    ) -> EnqueuedCommit {
        self.enqueue_commit_inner(
            CommitRequest {
                mode,
                record,
                events,
                semantics: CommitSemantics::default(),
                conversation_authority: Some(authority),
            },
            None,
            |_| {},
        )
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
        self.enqueue_commit_inner(
            CommitRequest {
                mode,
                record,
                events,
                semantics: CommitSemantics {
                    owned_fields,
                    ..CommitSemantics::default()
                },
                conversation_authority: None,
            },
            None,
            after_allocation,
        )
    }

    fn enqueue_commit_with_rotation<F>(
        self: &Arc<Self>,
        mode: CommitMode,
        record: SessionRecord,
        events: Vec<Event>,
        owned_fields: RecordFields,
        rotation_precondition: RotationPrecondition,
        after_allocation: F,
    ) -> EnqueuedCommit
    where
        F: FnOnce(u64),
    {
        self.enqueue_commit_inner(
            CommitRequest {
                mode,
                record,
                events,
                semantics: CommitSemantics {
                    owned_fields: Some(owned_fields),
                    rotation_precondition: Some(rotation_precondition),
                    terminal_lifecycle_repair: false,
                },
                conversation_authority: None,
            },
            None,
            after_allocation,
        )
    }

    fn enqueue_retained_commit<F>(
        self: &Arc<Self>,
        request: CommitRequest,
        retained: Box<dyn RetainedCommitWork>,
        after_allocation: F,
    ) where
        F: FnOnce(u64),
    {
        let enqueued = self.enqueue_commit_inner(request, Some(retained), after_allocation);
        drop(enqueued.complete);
    }

    fn enqueue_restartable_commit(
        self: &Arc<Self>,
        record: SessionRecord,
        events: Vec<Event>,
    ) -> RestartableCommit {
        self.enqueue_restartable_commit_with_authority(record, events, None)
    }

    fn enqueue_restartable_commit_with_authority(
        self: &Arc<Self>,
        mut record: SessionRecord,
        mut events: Vec<Event>,
        conversation_authority: Option<ConversationWriteAuthority>,
    ) -> RestartableCommit {
        let (outcome, receiver) = oneshot::channel();
        if record.session.id != self.id || events.iter().any(|event| event.session_id != self.id) {
            let _ = outcome.send(Err(StoreError::Malformed(
                "one retained cleanup write named more than one session".into(),
            )));
            return RestartableCommit {
                lane: Arc::clone(self),
                outcome: LaneReceiver::new(receiver, Arc::clone(self)),
            };
        }
        let appended_at = crate::clock::now_rfc3339();
        for event in &mut events {
            if event.appended_at.is_empty() {
                event.appended_at = appended_at.clone();
            }
        }

        let start = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !events.is_empty() {
                let count = u64::try_from(events.len()).unwrap_or(u64::MAX);
                let first = match self
                    .sequences
                    .try_take(&self.id, record.session.last_seq, count)
                {
                    Ok(first) => first,
                    Err(error) => {
                        let _ = outcome.send(Err(error));
                        return RestartableCommit {
                            lane: Arc::clone(self),
                            outcome: LaneReceiver::new(receiver, Arc::clone(self)),
                        };
                    }
                };
                if let Err(error) = renumber(&self.id, &mut events, first) {
                    let _ = outcome.send(Err(error));
                    return RestartableCommit {
                        lane: Arc::clone(self),
                        outcome: LaneReceiver::new(receiver, Arc::clone(self)),
                    };
                }
                record.session.last_seq = events
                    .last()
                    .map_or(record.session.last_seq, |event| event.seq);
            }
            state.queued.push_back(PendingOperation::RestartableCommit {
                write_id: format!("sesw_{}", uuid::Uuid::new_v4().simple()),
                record: Box::new(record),
                events,
                conversation_authority,
                outcome: Some(outcome),
                progress: CommitProgress::default(),
            });
            if state.running {
                false
            } else {
                state.running = true;
                true
            }
        };
        if start {
            self.spawn_worker();
        }
        RestartableCommit {
            lane: Arc::clone(self),
            outcome: LaneReceiver::new(receiver, Arc::clone(self)),
        }
    }

    fn enqueue_commit_inner<F>(
        self: &Arc<Self>,
        request: CommitRequest,
        mut retained: Option<Box<dyn RetainedCommitWork>>,
        after_allocation: F,
    ) -> EnqueuedCommit
    where
        F: FnOnce(u64),
    {
        let CommitRequest {
            mode,
            mut record,
            mut events,
            semantics,
            conversation_authority,
        } = request;
        let (complete, receiver) = oneshot::channel();
        let (acknowledge_error, observed) = oneshot::channel();
        if record.session.id != self.id || events.iter().any(|event| event.session_id != self.id) {
            let error = StoreError::Malformed(
                "one durable session write named more than one session".into(),
            );
            if let Some(retained) = retained.take() {
                self.enqueue(PendingOperation::RetainedCompletion {
                    retained,
                    outcome: Box::new(Err(error)),
                });
            } else {
                let _ = complete.send(Err(error));
            }
            return EnqueuedCommit {
                complete: CommitReceiver::new(receiver, Some(acknowledge_error), Arc::clone(self)),
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
                        if let Some(retained) = retained.take() {
                            state
                                .queued
                                .push_back(PendingOperation::RetainedCompletion {
                                    retained,
                                    outcome: Box::new(Err(error)),
                                });
                            let start = if state.running {
                                false
                            } else {
                                state.running = true;
                                true
                            };
                            drop(state);
                            if start {
                                self.spawn_worker();
                            }
                        } else {
                            let _ = complete.send(Err(error));
                        }
                        return EnqueuedCommit {
                            complete: CommitReceiver::new(
                                receiver,
                                Some(acknowledge_error),
                                Arc::clone(self),
                            ),
                            last_seq: record.session.last_seq,
                        };
                    }
                };
                if let Err(error) = renumber(&self.id, &mut events, first) {
                    if let Some(retained) = retained.take() {
                        state
                            .queued
                            .push_back(PendingOperation::RetainedCompletion {
                                retained,
                                outcome: Box::new(Err(error)),
                            });
                        let start = if state.running {
                            false
                        } else {
                            state.running = true;
                            true
                        };
                        drop(state);
                        if start {
                            self.spawn_worker();
                        }
                    } else {
                        let _ = complete.send(Err(error));
                    }
                    return EnqueuedCommit {
                        complete: CommitReceiver::new(
                            receiver,
                            Some(acknowledge_error),
                            Arc::clone(self),
                        ),
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
                        let fields = semantics
                            .owned_fields
                            .unwrap_or_else(|| RecordFields::for_commit(&before, &record, &events));
                        let change = Box::new(RecordChange {
                            before,
                            fields,
                            rotation_precondition: semantics.rotation_precondition,
                            terminal_lifecycle_repair: semantics.terminal_lifecycle_repair,
                        });
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
                conversation_authority,
                target: Some(match retained.take() {
                    Some(retained) => CommitTarget::Retained(retained),
                    None => CommitTarget::Ordinary {
                        complete: Some(complete),
                        observed: Some(observed),
                    },
                }),
                progress: CommitProgress::default(),
                outcome: None,
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
            self.spawn_worker();
        }
        EnqueuedCommit {
            complete: CommitReceiver::new(receiver, Some(acknowledge_error), Arc::clone(self)),
            last_seq,
        }
    }

    fn enqueue_delete(self: &Arc<Self>) -> LaneReceiver<Result<(), StoreError>> {
        let (complete, receiver) = oneshot::channel();
        self.enqueue(PendingOperation::Delete {
            complete: Some(complete),
            progress: DeleteProgress::default(),
        });
        LaneReceiver::new(receiver, Arc::clone(self))
    }

    /// Wait behind every operation already in this lane.
    pub(crate) async fn fence(self: &Arc<Self>) {
        let (complete, receiver) = oneshot::channel();
        self.enqueue(PendingOperation::Fence {
            complete: Some(complete),
        });
        let _ = LaneReceiver::new(receiver, Arc::clone(self)).await;
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
            self.spawn_worker();
        }
    }

    fn spawn_worker(self: &Arc<Self>) {
        let worker = UnpolledLaneWorker {
            lane: Arc::clone(self),
            started: false,
        };
        tokio::spawn(async move { worker.run().await });
    }

    fn kick(self: &Arc<Self>) {
        let start = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.running || state.queued.is_empty() {
                false
            } else {
                state.running = true;
                true
            }
        };
        if start {
            self.spawn_worker();
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

            let mut running = RunningOperation {
                lane: Arc::clone(&self),
                operation: Some(operation),
            };
            #[cfg(test)]
            if self.pause_after_pop.swap(false, Ordering::SeqCst) {
                self.operation_popped.notify_one();
                self.release_popped_operation.notified().await;
            }

            match running.operation_mut() {
                PendingOperation::Commit {
                    write_id,
                    mode,
                    record,
                    change,
                    events,
                    conversation_authority,
                    target,
                    progress,
                    outcome,
                } => {
                    let mode = *mode;
                    if outcome.is_none() {
                        let retain_after_cancellation = !matches!(mode, CommitMode::Create)
                            || matches!(target.as_ref(), Some(CommitTarget::Retained(_)));
                        let result = {
                            let mut reporter = CommitReporter {
                                progress,
                                target: target.as_mut(),
                                retain_after_cancellation,
                            };
                            self.commit(
                                CommitContext {
                                    write_id,
                                    mode,
                                    change: change.as_deref(),
                                    conversation_authority: conversation_authority.as_ref(),
                                },
                                record,
                                events,
                                &mut reporter,
                            )
                            .await
                        };
                        let result = result.map(|()| (**record).clone());
                        if let Err(error) = &result {
                            let retained =
                                if matches!(target.as_ref(), Some(CommitTarget::Retained(_))) {
                                    let Some(CommitTarget::Retained(retained)) = target.take()
                                    else {
                                        unreachable!("the retained target was just inspected")
                                    };
                                    Some((retained, error.clone()))
                                } else {
                                    None
                                };
                            self.finalize_rejected_commit(
                                events,
                                matches!(mode, CommitMode::UpdateInheriting),
                                retained,
                            );
                        }
                        *outcome = Some(Box::new(result));
                    }
                    let result = outcome
                        .as_ref()
                        .expect("a finished durable commit keeps its outcome")
                        .as_ref()
                        .clone();
                    if target.is_some() {
                        match target
                            .as_mut()
                            .expect("a commit keeps its target until completion")
                        {
                            CommitTarget::Ordinary { complete, .. } => {
                                if let Some(complete) = complete.take() {
                                    if let Err(Err(error)) =
                                        complete.send(result.map(CommitDelivery::plain))
                                    {
                                        tracing::error!(
                                            session_id = %self.id,
                                            error = %error,
                                            "a detached durable session write failed"
                                        );
                                    }
                                }
                            }
                            CommitTarget::Retained(retained) => {
                                retained.prepare_outcome(result);
                                retained.finish().await;
                            }
                        }
                    }
                }
                PendingOperation::RestartableCommit {
                    write_id,
                    record,
                    events,
                    conversation_authority,
                    outcome,
                    progress,
                } => {
                    let result = {
                        let mut reporter = CommitReporter {
                            progress,
                            target: None,
                            retain_after_cancellation: true,
                        };
                        self.commit(
                            CommitContext {
                                write_id,
                                mode: CommitMode::UpdateExact,
                                change: None,
                                conversation_authority: conversation_authority.as_ref(),
                            },
                            record,
                            events,
                            &mut reporter,
                        )
                        .await
                    };
                    if result.is_err() {
                        self.finalize_rejected_commit(events, false, None);
                    }
                    if let Some(outcome) = outcome.take() {
                        let _ = outcome.send(result.map(|()| (**record).clone()));
                    }
                }
                PendingOperation::Delete { complete, progress } => {
                    let result = self.delete(progress).await;
                    if let Some(complete) = complete.take() {
                        let _ = complete.send(result);
                    }
                }
                PendingOperation::ObserveRecord(record) => {
                    if let Some(record) = record.take() {
                        self.remember_durable(*record);
                    }
                }
                PendingOperation::Fence { complete } => {
                    if let Some(complete) = complete.take() {
                        let _ = complete.send(());
                    }
                }
                PendingOperation::RetainedCompletion { retained, outcome } => {
                    retained.prepare_outcome((**outcome).clone());
                    retained.finish().await;
                }
                #[cfg(test)]
                PendingOperation::ObserveRevision(revision) => {
                    self.revision.fetch_max(*revision, Ordering::AcqRel);
                }
            }
            running.finish();
        }
    }

    async fn commit(
        &self,
        context: CommitContext<'_>,
        record: &mut SessionRecord,
        events: &mut [Event],
        reporter: &mut CommitReporter<'_>,
    ) -> Result<(), StoreError> {
        let CommitContext {
            write_id,
            mode,
            change,
            conversation_authority,
        } = context;
        self.resolve_reported_error(reporter).await?;
        if matches!(mode, CommitMode::UpdateInheriting) && !reporter.progress.prepared {
            self.prepare_inheriting_record(record, change)?;
            reporter.progress.prepared = true;
        }
        loop {
            reporter.progress.attempt = reporter.progress.attempt.saturating_add(1);
            let attempt = reporter.progress.attempt;
            if let Some(last) = events.last() {
                record.session.last_seq = last.seq;
            }

            let result = match (mode, conversation_authority) {
                (CommitMode::Create, Some(_)) => Err(StoreError::Malformed(
                    "a conversation authority cannot create a session".into(),
                )),
                (CommitMode::UpdateExact | CommitMode::UpdateInheriting, Some(authority)) => {
                    self.store
                        .commit_conversation_idempotent(write_id, authority, record, events)
                        .await
                }
                (CommitMode::Create, None) => {
                    self.store
                        .create_session_idempotent(write_id, record, events)
                        .await
                }
                (CommitMode::UpdateExact | CommitMode::UpdateInheriting, None) => {
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
                    if self.durably_terminated().await
                        && !only_terminal_detach(events)
                        && !change.is_some_and(|change| change.terminal_lifecycle_repair)
                    {
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
                    if self.durably_terminated().await
                        && !only_terminal_detach(events)
                        && !change.is_some_and(|change| change.terminal_lifecycle_repair)
                    {
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
                            reporter.progress.unavailable_attempt =
                                reporter.progress.unavailable_attempt.saturating_add(1);
                            self.retry_or_stop(error, reporter).await?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error @ StoreError::Unavailable(_)) => {
                    reporter.progress.unavailable_attempt =
                        reporter.progress.unavailable_attempt.saturating_add(1);
                    self.retry_or_stop(error, reporter).await?;
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
                PendingOperation::Commit { events, .. }
                | PendingOperation::RestartableCommit { events, .. } => u64::try_from(events.len())
                    .map_err(|_| StoreError::Malformed("event batch is too large".into()))?,
                PendingOperation::Delete { .. }
                | PendingOperation::Fence { .. }
                | PendingOperation::RetainedCompletion { .. }
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
            let (record, events) = match operation {
                PendingOperation::Commit { record, events, .. }
                | PendingOperation::RestartableCommit { record, events, .. } => (record, events),
                _ => continue,
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

    /// Atomically remove a rejected proposal and order its retained cleanup.
    ///
    /// Successors already queued are shifted and have their lineage rebuilt
    /// before cleanup is appended. A producer arriving after this method takes
    /// the lane lock can therefore only enqueue behind that cleanup.
    fn finalize_rejected_commit(
        &self,
        events: &[Event],
        rebuild_local_lineage: bool,
        retained: Option<(Box<dyn RetainedCommitWork>, StoreError)>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !events.is_empty() {
            self.abandon_sequence_reservation_locked(&mut state, events);
        }
        if rebuild_local_lineage {
            Self::rebuild_local_lineage_locked(&mut state);
        }

        #[cfg(test)]
        if let Some(pause) = self
            .final_rejection_pause
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = pause.reached.send(());
            let _ = pause.release.recv();
        }

        // Rejection repair observes every successor rebuilt above. It remains
        // ahead of producers that arrive after this locked linearization point.
        if let Some((retained, error)) = retained {
            state
                .queued
                .push_back(PendingOperation::RetainedCompletion {
                    retained,
                    outcome: Box::new(Err(error)),
                });
        }
    }

    /// Remove one failed unit's reservation and shift every successor with it.
    fn abandon_sequence_reservation_locked(&self, state: &mut LaneState, events: &[Event]) {
        let durable_floor = self.sequences.durable_floor(&self.id);
        // A store-authored event can take a position already reserved by this
        // rejected unit. That occupied prefix is not abandoned: only its
        // reservations above the durable floor can be returned and removed
        // from queued successors.
        let count = events
            .iter()
            .filter(|event| event.seq > durable_floor)
            .count();
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        for operation in &mut state.queued {
            let (mode, record, events) = match operation {
                PendingOperation::Commit {
                    mode,
                    record,
                    events,
                    ..
                } => (*mode, record, events),
                PendingOperation::RestartableCommit { record, events, .. } => {
                    (CommitMode::UpdateExact, record, events)
                }
                _ => continue,
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
    fn rebuild_local_lineage_locked(state: &mut LaneState) {
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
                    let (fields, rotation_precondition, terminal_lifecycle_repair) =
                        change.as_deref().map_or_else(
                            || {
                                (
                                    RecordFields::for_commit(&before, record, events),
                                    None,
                                    false,
                                )
                            },
                            |change| {
                                (
                                    change.fields,
                                    change.rotation_precondition.clone(),
                                    change.terminal_lifecycle_repair,
                                )
                            },
                        );
                    let rebuilt = Box::new(RecordChange {
                        before,
                        fields,
                        rotation_precondition,
                        terminal_lifecycle_repair,
                    });
                    local_tail = Some(Box::new(rebuilt.next_local_tail(record)));
                    *change = Some(rebuilt);
                }
                PendingOperation::ObserveRecord(record) => {
                    if let Some(record) = record.as_ref() {
                        local_tail = Some(record.clone());
                    }
                }
                PendingOperation::Commit { .. }
                | PendingOperation::RestartableCommit { .. }
                | PendingOperation::Delete { .. }
                | PendingOperation::Fence { .. }
                | PendingOperation::RetainedCompletion { .. } => {}
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
        if durable.session.status.is_terminal()
            && !only_terminal_detach(events)
            && !change.is_some_and(|change| change.terminal_lifecycle_repair)
        {
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

    async fn resolve_reported_error(
        &self,
        reporter: &mut CommitReporter<'_>,
    ) -> Result<(), StoreError> {
        let Some(error) = reporter.progress.reported_error.clone() else {
            return Ok(());
        };
        if reporter.error_observed().await {
            return Err(error);
        }

        reporter.clear_error_observation();
        reporter.progress.reported_error = None;
        if reporter.retain_after_cancellation {
            Ok(())
        } else {
            Err(error)
        }
    }

    async fn retry_or_stop(
        &self,
        error: StoreError,
        reporter: &mut CommitReporter<'_>,
    ) -> Result<(), StoreError> {
        let unavailable_attempt = reporter.progress.unavailable_attempt;
        if unavailable_attempt == 1 || unavailable_attempt.is_power_of_two() {
            tracing::warn!(
                session_id = %self.id,
                attempt = unavailable_attempt,
                error = %error,
                "durable session write retained by its retry worker"
            );
        }
        if unavailable_attempt == REPORT_AFTER_ATTEMPTS {
            reporter.progress.reported_error = Some(error.clone());
            if reporter.report_error(error.clone()) {
                #[cfg(test)]
                self.failure_delivered.notify_one();
                return self.resolve_reported_error(reporter).await;
            }
            reporter.progress.reported_error = None;
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

    async fn delete(&self, progress: &mut DeleteProgress) -> Result<(), StoreError> {
        loop {
            match self.store.delete(&self.id).await {
                Ok(()) => {
                    let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                    state.durable = None;
                    state.local_tail = None;
                    return Ok(());
                }
                Err(error @ StoreError::Unavailable(_)) => {
                    progress.unavailable_attempt = progress.unavailable_attempt.saturating_add(1);
                    let unavailable_attempt = progress.unavailable_attempt;
                    if unavailable_attempt == 1 || unavailable_attempt.is_power_of_two() {
                        tracing::warn!(
                            session_id = %self.id,
                            attempt = unavailable_attempt,
                            error = %error,
                            "durable session deletion retained by its retry worker"
                        );
                    }
                    if unavailable_attempt == REPORT_AFTER_ATTEMPTS {
                        return Err(error);
                    }
                    retry_delay(unavailable_attempt).await;
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
/// A live entry, caller, worker, or queued retained continuation owns its lane
/// strongly. Retained work breaks its temporary self-cycle on completion. Dead
/// weak entries are pruned on every lookup, so touching one-off sessions cannot
/// grow this map without bound.
pub(crate) struct EventLogs {
    held: StdMutex<HashMap<SessionId, Weak<SessionLog>>>,
    store: Arc<dyn SessionStore>,
    sequences: Arc<Sequences>,
    cleanups: Arc<DeferredCleanups>,
}

impl EventLogs {
    pub(crate) fn new(store: Arc<dyn SessionStore>, sequences: Arc<Sequences>) -> Self {
        Self {
            held: StdMutex::new(HashMap::new()),
            store,
            sequences,
            cleanups: Arc::new(DeferredCleanups::default()),
        }
    }

    pub(crate) fn retain_cleanup(&self, cleanup: impl DeferredCleanup + 'static) {
        self.cleanups.retain(Box::new(cleanup));
    }

    fn cleanup_gate(&self, id: &SessionId) -> Arc<CleanupGate> {
        self.cleanups.drain_gate(id)
    }

    async fn drain_cleanups_at(&self, id: &SessionId, gate: &CleanupGate) {
        self.cleanups.drain_at(id, gate).await;
    }

    /// Resume service-owned cleanup before retrying an operation it may fence.
    pub(crate) async fn drain_retained_cleanups(&self, id: &SessionId) {
        let gate = self.cleanup_gate(id);
        self.drain_cleanups_at(id, &gate).await;
    }

    fn cleanup_pending(&self, id: &SessionId) -> bool {
        self.cleanups.has_pending(id)
    }

    #[cfg(test)]
    pub(crate) fn cleanup_drain_users(&self, id: &SessionId) -> usize {
        self.cleanups.drain_users(id)
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
    pub(crate) async fn lock_lifecycle(&self, id: &SessionId) -> tokio::sync::OwnedMutexGuard<()> {
        let event_log = self.event_logs.lane(id);
        let cleanup_gate = self.event_logs.cleanup_gate(id);
        let lifecycle_gate = self.lifecycle_gates.gate(id);
        loop {
            // Register before `kick`: a worker can stop after the state check,
            // and `notify_waiters` deliberately stores no permit for a listener
            // that did not exist yet.
            let restart_needed = event_log.restart_needed.notified();
            tokio::pin!(restart_needed);
            restart_needed.as_mut().enable();
            // A retained continuation can own this gate after its original
            // runtime and receiver disappear. Public lifecycle work is itself
            // a recovery trigger for that weakly registered lane.
            event_log.kick();
            self.event_logs.drain_cleanups_at(id, &cleanup_gate).await;

            // Register before inspecting the queue. A delivery dropped from a
            // non-runtime thread can otherwise arrive after the empty drain and
            // retain the lifecycle guard without any task available to run it.
            let changed = cleanup_gate.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.event_logs.cleanup_pending(id) {
                continue;
            }

            tokio::select! {
                lifecycle = Arc::clone(&lifecycle_gate).lock_owned() => {
                    if self.event_logs.cleanup_pending(id) {
                        drop(lifecycle);
                        continue;
                    }
                    return lifecycle;
                }
                () = &mut changed => {}
                () = &mut restart_needed => {}
            }
        }
    }

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

    /// Persist only a custody-state transition over unrelated durable changes.
    ///
    /// Custody moves without a public event while bytes cross from the session
    /// row to object storage. Empty ordinary commits are intentionally fieldless,
    /// so this narrow writer names the one field it owns explicitly.
    pub(crate) async fn persist_skill_pack_state_durably(
        &self,
        record: &mut SessionRecord,
        state: crate::store::SkillPackState,
    ) -> Result<(), StoreError> {
        if record.skill_packs == state {
            return Ok(());
        }
        let before = record.clone();
        record.skill_packs = state;
        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(&before);
        let result = lane
            .enqueue_commit_with(
                CommitMode::UpdateInheriting,
                record.clone(),
                Vec::new(),
                Some(RecordFields {
                    skill_packs: true,
                    ..RecordFields::default()
                }),
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Persist an exact proposal under one durable conversation predicate.
    pub(crate) async fn persist_conversation_durably(
        &self,
        record: &mut SessionRecord,
        events: Vec<Event>,
        authority: ConversationWriteAuthority,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        let result = lane
            .enqueue_conversation_commit(CommitMode::UpdateExact, record.clone(), events, authority)
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Complete a handover while preserving history written after its claim.
    ///
    /// The claim itself is the compare precondition. A later handover has a
    /// different value even when it happens to target the same runtime and
    /// lifecycle state, so an obsolete task cannot win an ABA race.
    pub(crate) async fn complete_handover_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        // A restarted service may not have a predecessor in this process's
        // lane yet. Seed the exact durable claim before constructing the
        // inheriting change so conflict rebase cannot drop its authority
        // predicate and fall back to a generic semantic replacement.
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                record.clone(),
                events,
                RecordFields {
                    status: true,
                    geometry: true,
                    plane: true,
                    runtime: true,
                    rotation: true,
                    skill_grant: true,
                    skill_packs: true,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: None,
                    pending_handover: None,
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Claim one handover and, for legacy sessions, stage its exact H2 bytes.
    ///
    /// History may advance while preflight reads object storage. The strict
    /// runtime/lifecycle predicate lets this unit inherit that history without
    /// separating the noninteractive boundary from the candidate it protects.
    pub(crate) async fn mark_handover_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                record.clone(),
                events,
                RecordFields {
                    status: true,
                    rotation: true,
                    handover: true,
                    skill_packs: claimed.skill_packs != record.skill_packs,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: Some(claimed.runtime_id.clone()),
                    pending_handover: Some(claimed.pending_handover.clone()),
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Commit a terminal lifecycle boundary together with skill cleanup intent.
    ///
    /// Runtime retirement happens under a Lifecycle lease first. This unit then
    /// owns the exact status/claim/custody fields it settles while inheriting
    /// unrelated history that raced the provider operation.
    pub(crate) async fn terminate_with_skill_cleanup_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                record.clone(),
                events,
                RecordFields {
                    status: true,
                    rotation: true,
                    handover: true,
                    skill_packs: true,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: Some(claimed.runtime_id.clone()),
                    pending_handover: Some(claimed.pending_handover.clone()),
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Publish one ordinary or compatibility runtime's Ready boundary.
    ///
    /// The skill grant and custody record become authoritative only with the
    /// exact runtime that receives those bytes. They are therefore explicit
    /// fields of this unit rather than inferred from an arbitrary status event.
    pub(crate) async fn publish_ready_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(claimed);
        let fields = RecordFields {
            status: true,
            geometry: true,
            plane: true,
            // Readoption may mint the compatibility claim that fences local
            // publication. Ordinary provisioning carries its existing claim
            // unchanged, so owning this field is a no-op there.
            rotation: true,
            skill_grant: true,
            skill_packs: true,
            ..RecordFields::default()
        };
        // A claimless compatibility readoption mints its claim in this write.
        // The old absence is still a strict predicate: otherwise conflict
        // rebase could inherit a concurrently switched runtime and publish the
        // stale runtime's Ready grant/custody against its successor.
        let enqueued = lane.enqueue_commit_with_rotation(
            CommitMode::UpdateInheriting,
            record.clone(),
            events,
            fields,
            RotationPrecondition {
                claim: claimed.rotation_claim.clone(),
                status: claimed.session.status,
                runtime_id: Some(claimed.runtime_id.clone()),
                pending_handover: Some(claimed.pending_handover.clone()),
            },
            |_| {},
        );
        let result = enqueued
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Publish compatibility handover semantics after the old runtime is gone.
    ///
    /// Unlike coordinated completion, this stage must never race ahead of H1
    /// cleanup. The exact empty runtime boundary is therefore part of the same
    /// strict rotation predicate as the claim and lifecycle status.
    pub(crate) async fn complete_compatibility_handover_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        if !claimed.session.status.is_rotating()
            || claimed.rotation_claim.is_none()
            || claimed.runtime_id.is_some()
            || claimed
                .pending_handover
                .as_ref()
                .map(|pending| pending.claim.as_str())
                != claimed.rotation_claim.as_deref()
        {
            return Err(StoreError::Malformed(
                "a compatibility handover requires one cleared rotating claim".into(),
            ));
        }
        if record.session.id != claimed.session.id
            || record.session.status != claimed.session.status
            || record.rotation_claim != claimed.rotation_claim
            || record.pending_handover != claimed.pending_handover
            || record.runtime_id.is_some()
        {
            return Err(StoreError::Malformed(
                "a compatibility handover changed its cleared authority boundary".into(),
            ));
        }

        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                record.clone(),
                events,
                RecordFields {
                    status: true,
                    geometry: true,
                    plane: true,
                    runtime: true,
                    rotation: true,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: Some(None),
                    pending_handover: Some(claimed.pending_handover.clone()),
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Clear one published handover's exact durable claim and descriptor.
    ///
    /// Publication can happen after history and conversation status advance,
    /// so this is an inheriting write that owns both private handover fields.
    /// The claim, descriptor, and freshly read status remain strict
    /// preconditions: an obsolete task cannot clear a later rotation after an
    /// ABA transition.
    pub(crate) async fn clear_rotation_claim_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
    ) -> Result<(), StoreError> {
        if claimed.rotation_claim.is_none()
            || claimed.runtime_id.is_none()
            || claimed.pending_handover.as_ref().is_some_and(|pending| {
                Some(pending.claim.as_str()) != claimed.rotation_claim.as_deref()
            })
            || record.pending_handover.is_some()
        {
            return Err(StoreError::Malformed(
                "a published rotation claim requires an exact runtime".into(),
            ));
        }
        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                record.clone(),
                Vec::new(),
                RecordFields {
                    rotation: true,
                    handover: true,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: Some(claimed.runtime_id.clone()),
                    pending_handover: Some(claimed.pending_handover.clone()),
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Clear one compatibility runtime without replacing unrelated durable state.
    ///
    /// Compatibility cleanup has no coordination lease to fence successors, so
    /// the exact lifecycle state, rotation claim, and runtime observed before
    /// provider deletion form its authority boundary.
    pub(crate) async fn clear_compatibility_runtime_durably(
        &self,
        intended: &mut SessionRecord,
        claimed: &SessionRecord,
    ) -> Result<(), StoreError> {
        if claimed.runtime_id.is_none() || intended.runtime_id.is_some() {
            return Err(StoreError::Malformed(
                "a compatibility runtime clear requires one exact runtime".into(),
            ));
        }
        if intended.session.id != claimed.session.id
            || intended.session.status != claimed.session.status
            || intended.rotation_claim != claimed.rotation_claim
            || intended.pending_handover != claimed.pending_handover
            || claimed.pending_handover.as_ref().is_some_and(|pending| {
                Some(pending.claim.as_str()) != claimed.rotation_claim.as_deref()
            })
        {
            return Err(StoreError::Malformed(
                "a compatibility runtime clear changed its authority boundary".into(),
            ));
        }

        let lane = self.event_logs.lane(&intended.session.id);
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                intended.clone(),
                Vec::new(),
                RecordFields {
                    runtime: true,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: Some(claimed.runtime_id.clone()),
                    pending_handover: Some(claimed.pending_handover.clone()),
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *intended = result;
        Ok(())
    }

    /// Restore the old runtime only while this task still owns the handover.
    pub(crate) async fn roll_back_handover_durably(
        &self,
        record: &mut SessionRecord,
        claimed: &SessionRecord,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let lane = self.event_logs.lane(&record.session.id);
        lane.observe_record(claimed);
        let result = lane
            .enqueue_commit_with_rotation(
                CommitMode::UpdateInheriting,
                record.clone(),
                events,
                RecordFields {
                    status: true,
                    rotation: true,
                    handover: true,
                    skill_packs: true,
                    ..RecordFields::default()
                },
                RotationPrecondition {
                    claim: claimed.rotation_claim.clone(),
                    status: claimed.session.status,
                    runtime_id: None,
                    pending_handover: Some(claimed.pending_handover.clone()),
                },
                |_| {},
            )
            .complete
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))??;
        *record = result;
        Ok(())
    }

    /// Persist an exact proposal whose required side effects outlive its waiter.
    pub(crate) fn persist_durably_with<T, H, F, Fut, R, RFut, U>(
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
        U: FnOnce(T, H) + Send + 'static,
    {
        let id = record.session.id.clone();
        let lane = self.event_logs.lane(&id);
        let (retained, complete) = retain_commit(
            id,
            Arc::clone(&lane),
            continuation,
            on_rejected,
            on_undelivered,
        );
        lane.enqueue_retained_commit(
            CommitRequest {
                mode: CommitMode::UpdateExact,
                record,
                events,
                semantics: CommitSemantics::default(),
                conversation_authority: None,
            },
            retained,
            |_| {},
        );
        complete
    }

    /// Retain a compatibility terminal repair behind its exact observed boundary.
    ///
    /// Skill cleanup owns only custody state and may advance the revision after
    /// termination has committed. This repair may inherit that disjoint change,
    /// but the observed status, runtime, rotation claim, and handover descriptor
    /// must all still match before it clears the residual lifecycle fields.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn persist_terminal_lifecycle_repair_with<T, H, F, Fut, R, RFut, U>(
        &self,
        claimed: &SessionRecord,
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
        U: FnOnce(T, H) + Send + 'static,
    {
        let id = record.session.id.clone();
        let lane = self.event_logs.lane(&id);
        lane.observe_record(claimed);
        let (retained, complete) = retain_commit(
            id,
            Arc::clone(&lane),
            continuation,
            on_rejected,
            on_undelivered,
        );
        lane.enqueue_retained_commit(
            CommitRequest {
                mode: CommitMode::UpdateInheriting,
                record,
                events,
                semantics: CommitSemantics {
                    owned_fields: Some(RecordFields {
                        status: true,
                        rotation: true,
                        handover: true,
                        ..RecordFields::default()
                    }),
                    rotation_precondition: Some(RotationPrecondition {
                        claim: claimed.rotation_claim.clone(),
                        status: claimed.session.status,
                        runtime_id: Some(claimed.runtime_id.clone()),
                        pending_handover: Some(claimed.pending_handover.clone()),
                    }),
                    terminal_lifecycle_repair: true,
                },
                conversation_authority: None,
            },
            retained,
            |_| {},
        );
        complete
    }

    pub(crate) fn retain_cleanup_durably(
        &self,
        record: SessionRecord,
        events: Vec<Event>,
    ) -> RestartableCommit {
        self.event_logs
            .lane(&record.session.id)
            .enqueue_restartable_commit(record, events)
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

        self.synchronize_ordered_live_tail_for(id, screen).await;
    }

    /// Repair a live cursor from a rejection cleanup already ordered in its lane.
    pub(crate) async fn synchronize_ordered_live_tail_for(
        &self,
        id: &SessionId,
        screen: &Arc<tokio::sync::Mutex<Box<dyn crate::terminal::ScreenModel>>>,
    ) {
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(id) else {
            return;
        };
        if Arc::ptr_eq(&entry.screen, screen) {
            let event_log = Arc::clone(&entry.event_log);
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

    /// Wait for every earlier update from one exact conversation link.
    ///
    /// The empty barrier carries the same durable epoch as the events ahead of
    /// it. A stale replica therefore observes revocation instead of reporting a
    /// successful flush after all of its event-bearing writes were refused.
    pub(crate) async fn flush_conversation(
        &self,
        id: &SessionId,
        link: crate::conversation_opening::ConversationLinkId,
    ) -> bool {
        let (receiver, authority) = {
            let entries = self.entries.read().await;
            let Some(entry) = entries
                .get(id)
                .filter(|entry| entry.conversation_link == Some(link))
            else {
                return false;
            };
            let Some(authority) = entry.conversation_authority.clone() else {
                return false;
            };
            let receiver = entry
                .event_log
                .enqueue_conversation_commit(
                    CommitMode::UpdateInheriting,
                    record_from_entry(entry),
                    Vec::new(),
                    ConversationWriteAuthority::Active(authority.clone()),
                )
                .complete;
            (receiver, authority)
        };
        match receiver
            .await
            .map_err(|_| StoreError::Unavailable("session write worker stopped".into()))
            .and_then(|result| result)
        {
            Ok(record) => {
                if let Some(entry) = self
                    .entries
                    .write()
                    .await
                    .get_mut(id)
                    .filter(|entry| entry.conversation_link == Some(link))
                {
                    let event_log = Arc::clone(&entry.event_log);
                    entry.session.last_seq = record.session.last_seq;
                    event_log.synchronize_tail(&mut entry.session.last_seq);
                }
                true
            }
            Err(error @ StoreError::ConversationLinkChanged { .. }) => {
                self.retire_conversation_link(id, link, &authority).await;
                tracing::debug!(session_id = %id, %error, "stale conversation link write was fenced");
                false
            }
            Err(error) => {
                self.retire_conversation_link(id, link, &authority).await;
                tracing::error!(session_id = %id, error = %error, "cannot persist conversation");
                false
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
    record.skill_packs = entry.skill_packs.clone();
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
            || before.session.pricing != intended.session.pricing
            || before.session.created_at != intended.session.created_at;
        // Runtime authority belongs only to an event whose semantics name it or
        // to a workflow that supplies explicit narrow ownership. A queued
        // history event can carry an old runtime snapshot after termination or
        // handover; a mere difference from its local predecessor must never let
        // it resurrect that handle over newer durable authority.
        fields.context = before.context_ref != intended.context_ref;
        // Skill grants and byte custody are also explicit-only. A queued event
        // carries a full live snapshot that may predate handover or cleanup;
        // inferring either field here would let that stale snapshot roll a
        // Ready grant or terminal cleanup state backward.
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
                // The compatibility marker is committed atomically with the
                // handle it names. Coordinated starts are inserted by the
                // coordination store itself; the handover marker rides beside
                // a status event that owns the lifecycle mutation.
                EventKind::RuntimeIncarnationStarted => fields.runtime = true,
                EventKind::RuntimeHandoverStarted | EventKind::RuntimeStarted { .. } => {}
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
            || (self.fields.status && !intended.session.wants_runtime());
        // A rotation claim is also a strict lifecycle precondition. Unlike an
        // ordinary selected field, an unchanged Rotating status is meaningful:
        // another writer that settled the session must invalidate this task
        // even if an older writer did not know to clear the new claim field.
        let rotation_before = self
            .rotation_precondition
            .as_ref()
            .map_or(&before.rotation_claim, |required| &required.claim);
        let mut compatible = self.rotation_precondition.as_ref().is_none_or(|required| {
            durable.rotation_claim == required.claim
                && durable.session.status == required.status
                && required
                    .runtime_id
                    .as_ref()
                    .is_none_or(|runtime| &durable.runtime_id == runtime)
                && required
                    .pending_handover
                    .as_ref()
                    .is_none_or(|handover| &durable.pending_handover == handover)
        });

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
            self.fields.skill_grant,
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
        compatible &= rebase_selected(
            self.fields.rotation,
            &mut durable.rotation_claim,
            rotation_before,
            &intended.rotation_claim,
        );
        compatible &= rebase_selected(
            self.fields.handover,
            &mut durable.pending_handover,
            &before.pending_handover,
            &intended.pending_handover,
        );
        compatible &= rebase_selected(
            self.fields.skill_packs,
            &mut durable.skill_packs,
            &before.skill_packs,
            &intended.skill_packs,
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
            || (self.fields.status && !intended.session.wants_runtime());

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
            self.fields.skill_grant,
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
        copy_selected(
            self.fields.rotation,
            &mut target.rotation_claim,
            &intended.rotation_claim,
        );
        copy_selected(
            self.fields.handover,
            &mut target.pending_handover,
            &intended.pending_handover,
        );
        copy_selected(
            self.fields.skill_packs,
            &mut target.skill_packs,
            &intended.skill_packs,
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
            | EventKind::PlaneChanged { .. }
            | EventKind::RuntimeIncarnationStarted => {
                return Err(StoreError::RevisionConflict {
                    expected: intended.revision,
                    actual: durable.revision,
                });
            }
            // These accounting markers own no independent record mutation.
            EventKind::RuntimeHandoverStarted | EventKind::RuntimeStarted { .. } => {}
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

/// Record an inbound conversation event under the entry's durable link epoch.
pub(crate) fn append_conversation(entry: &mut SessionEntry, kind: EventKind) {
    let Some(authority) = entry.conversation_authority.clone() else {
        tracing::error!(
            session_id = %entry.session.id,
            "conversation entry has no durable link authority"
        );
        return;
    };
    let event = Event {
        session_id: entry.session.id.clone(),
        seq: 0,
        kind,
        appended_at: String::new(),
    };
    let record = record_from_entry(entry);
    let enqueued = entry.event_log.enqueue_conversation_commit(
        CommitMode::UpdateInheriting,
        record,
        vec![event],
        ConversationWriteAuthority::Active(authority),
    );
    entry.session.last_seq = enqueued.last_seq;
    drop(enqueued.complete);
}

/// Record one indivisible event unit and return its durable completion.
///
/// Some public operations need both properties at once: their related events
/// must never be interleaved by another replica, and the caller must not act on
/// an enqueue-time sequence that a normal [`StoreError::SequenceTaken`] rebase
/// can move. The completion record carries the unit's final committed tail.
pub(crate) fn append_batch(
    entry: &mut SessionEntry,
    kinds: impl IntoIterator<Item = EventKind>,
) -> CommitReceiver {
    let events = kinds
        .into_iter()
        .map(|kind| Event {
            session_id: entry.session.id.clone(),
            seq: 0,
            kind,
            appended_at: String::new(),
        })
        .collect::<Vec<_>>();
    debug_assert!(!events.is_empty(), "an event unit must contain an event");
    let record = record_from_entry(entry);
    let enqueued = entry
        .event_log
        .enqueue_commit(CommitMode::UpdateInheriting, record, events);
    // Publish the initial reservation to live producers. If another replica
    // wins a sequence first, the lane moves this unit and all successors as one
    // FIFO; the returned completion supplies the post-rebase durable tail.
    entry.session.last_seq = enqueued.last_seq;
    enqueued.complete
}

/// Queue a semantic event whose intended record is not live state until commit.
///
/// Public operations that act on a runtime use this form so a cross-replica
/// conflict can be reported before the runtime is changed. The ordinary
/// [`append`] path has already observed its state change and therefore leaves
/// the detached lane responsible for persistence after the caller moves on.
pub(crate) fn append_with<T, H, F, Fut, R, RFut, U>(
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
    U: FnOnce(T, H) + Send + 'static,
{
    append_with_authority(
        entry,
        kind,
        change,
        AppendPredicate::unconstrained(),
        continuation,
        on_rejected,
        on_undelivered,
    )
}

/// Queue a lifecycle event that atomically settles an observed rotation claim.
///
/// A live entry deliberately carries no private durable claim. The caller's
/// fresh record supplies that exact token and runtime boundary so stopping or
/// terminating a published replacement clears only the handover it observed,
/// never a later replica's claim.
pub(crate) fn append_with_rotation_claim<T, H, F, Fut, R, RFut, U>(
    entry: &mut SessionEntry,
    claimed: Option<&SessionRecord>,
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
    U: FnOnce(T, H) + Send + 'static,
{
    append_with_authority(
        entry,
        kind,
        change,
        claimed.map_or_else(AppendPredicate::unconstrained, AppendPredicate::rotation),
        continuation,
        on_rejected,
        on_undelivered,
    )
}

/// Queue the one terminal lifecycle repair allowed to inherit a terminal row.
///
/// This is deliberately separate from the ordinary rotation-aware append. Its
/// strict observed boundary still rejects a later lifecycle owner; only a
/// disjoint revision advance such as skill cleanup can be inherited.
pub(crate) fn append_terminal_lifecycle_repair<T, H, F, Fut, R, RFut, U>(
    entry: &mut SessionEntry,
    claimed: &SessionRecord,
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
    U: FnOnce(T, H) + Send + 'static,
{
    append_with_authority(
        entry,
        kind,
        change,
        AppendPredicate::terminal_repair(claimed),
        continuation,
        on_rejected,
        on_undelivered,
    )
}

/// Queue a semantic conversation event under its exact durable link fence.
///
/// The proposed record participates in inheriting lineage immediately, while
/// retained completion keeps it out of live state until durability. If the
/// link or lifecycle changes first, rejection rebuilds that lineage before a
/// later producer can enqueue behind it. Compatibility stores pass no durable
/// authority and retain the same process-local ordering behavior.
pub(crate) fn append_conversation_with<T, H, F, Fut, R, RFut, U>(
    entry: &mut SessionEntry,
    kind: EventKind,
    change: impl FnOnce(&mut SessionRecord),
    authority: Option<crate::coordination::ConversationLink>,
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
    U: FnOnce(T, H) + Send + 'static,
{
    append_with_authority(
        entry,
        kind,
        change,
        authority.map_or_else(AppendPredicate::unconstrained, |authority| {
            AppendPredicate::conversation(ConversationWriteAuthority::Active(authority))
        }),
        continuation,
        on_rejected,
        on_undelivered,
    )
}

struct AppendPredicate<'a> {
    authority: Option<ConversationWriteAuthority>,
    claimed_rotation: Option<&'a SessionRecord>,
    terminal_lifecycle_repair: bool,
}

impl<'a> AppendPredicate<'a> {
    fn unconstrained() -> Self {
        Self {
            authority: None,
            claimed_rotation: None,
            terminal_lifecycle_repair: false,
        }
    }

    fn conversation(authority: ConversationWriteAuthority) -> Self {
        Self {
            authority: Some(authority),
            claimed_rotation: None,
            terminal_lifecycle_repair: false,
        }
    }

    fn rotation(claimed: &'a SessionRecord) -> Self {
        Self {
            authority: None,
            claimed_rotation: Some(claimed),
            terminal_lifecycle_repair: false,
        }
    }

    fn terminal_repair(claimed: &'a SessionRecord) -> Self {
        Self {
            authority: None,
            claimed_rotation: Some(claimed),
            terminal_lifecycle_repair: true,
        }
    }
}

fn append_with_authority<T, H, F, Fut, R, RFut, U>(
    entry: &mut SessionEntry,
    kind: EventKind,
    change: impl FnOnce(&mut SessionRecord),
    predicate: AppendPredicate<'_>,
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
    U: FnOnce(T, H) + Send + 'static,
{
    let AppendPredicate {
        authority,
        claimed_rotation,
        terminal_lifecycle_repair,
    } = predicate;
    let event = Event {
        session_id: entry.session.id.clone(),
        seq: 0,
        kind,
        appended_at: String::new(),
    };
    let mut record = record_from_entry(entry);
    let rotation_precondition = claimed_rotation.map(|claimed| {
        // The live entry may predate a claim written by another replica. Start
        // this proposal at the exact revision just read by the lifecycle
        // caller; the entry still supplies its lane-authoritative sequence and
        // context state. Any write after that observation conflicts normally
        // and is then judged by the strict claim/status/runtime predicate below.
        record.revision = claimed.revision;
        record.rotation_claim.clone_from(&claimed.rotation_claim);
        record
            .pending_handover
            .clone_from(&claimed.pending_handover);
        record.runtime_id.clone_from(&claimed.runtime_id);
        RotationPrecondition {
            claim: claimed.rotation_claim.clone(),
            status: claimed.session.status,
            runtime_id: Some(claimed.runtime_id.clone()),
            pending_handover: Some(claimed.pending_handover.clone()),
        }
    });
    change(&mut record);
    let id = entry.session.id.clone();
    let event_log = Arc::clone(&entry.event_log);
    let (retained, complete) = retain_commit(
        id,
        Arc::clone(&event_log),
        continuation,
        on_rejected,
        on_undelivered,
    );
    if let Some(claimed) = claimed_rotation {
        // A claim may have been written by another replica after this live
        // entry was published. Put the exact durable snapshot in the lane
        // immediately before the lifecycle proposal so its semantic change is
        // measured from the claim it must settle, not from the stale entry.
        // Older queued work still runs first and a newer durable revision is
        // handled by the normal strict-precondition rebase.
        event_log.observe_record(claimed);
    }
    let mut live_tail = None;
    event_log.enqueue_retained_commit(
        CommitRequest {
            mode: CommitMode::UpdateInheriting,
            record,
            events: vec![event],
            semantics: match rotation_precondition {
                Some(rotation_precondition) => CommitSemantics {
                    owned_fields: Some(RecordFields {
                        status: true,
                        context: true,
                        rotation: true,
                        handover: true,
                        ..RecordFields::default()
                    }),
                    rotation_precondition: Some(rotation_precondition),
                    terminal_lifecycle_repair,
                },
                None => CommitSemantics::default(),
            },
            conversation_authority: authority,
        },
        retained,
        |last_seq| live_tail = Some(last_seq.saturating_sub(1)),
    );
    // A proposal is not live until its continuation installs the committed
    // record. Publish only the lane tail that preceded its reservation.
    if let Some(live_tail) = live_tail {
        entry.session.last_seq = live_tail;
    }
    complete
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod log_tests;

#[cfg(test)]
#[path = "flush_tests.rs"]
mod flush_tests;
