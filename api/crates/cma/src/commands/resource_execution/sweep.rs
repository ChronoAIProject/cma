use super::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use tokio::task::{Id, JoinError, JoinSet};
use tokio::time::Instant;

mod supervision;

const OPERATION_CONCURRENCY: usize = 4;
const SCAN_INTERVAL: Duration = Duration::from_millis(250);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

#[async_trait::async_trait]
pub(super) trait SweepWork: Send + Sync {
    async fn discover(&self, after: Option<&wire::Digest>) -> ExecutionResult<PendingPage>;
    async fn reconcile(&self, operation: &wire::Digest) -> ExecutionResult<Progress>;
}

#[async_trait::async_trait]
impl SweepWork for RevocationWorker {
    async fn discover(&self, after: Option<&wire::Digest>) -> ExecutionResult<PendingPage> {
        self.discover(after).await
    }
    async fn reconcile(&self, operation: &wire::Digest) -> ExecutionResult<Progress> {
        self.reconcile(operation).await
    }
}

pub(crate) enum SweepEvent {
    Discovered {
        count: usize,
        end_of_scan: bool,
    },
    DiscoveryFailed(ExecutionError),
    Reconciled {
        operation: wire::Digest,
        result: ExecutionResult<Progress>,
    },
    Waiting,
    SchedulingStopped,
}

enum TaskKind {
    Discovery,
    Reconciliation(wire::Digest),
}

enum TaskOutput {
    Discovery(ExecutionResult<PendingPage>),
    Reconciliation(ExecutionResult<Progress>),
}

enum Phase {
    Open,
    Closing { deadline: Instant, aborted: bool },
    Closed,
}

/// Own this driver outside the supervisor's cancellable scheduling future.
/// Construction starts no tasks; Mongo retains every operation's authority.
pub(crate) struct RevocationSweep {
    activity: Option<crate::worker_supervision::Activity>,
    reconciliation: Option<crate::worker_supervision::Reconciliation>,
    telemetry: Option<crate::telemetry::worker::Worker>,
    work: Arc<dyn SweepWork>,
    tasks: JoinSet<TaskOutput>,
    kinds: HashMap<Id, TaskKind>,
    queued: VecDeque<wire::Digest>,
    in_flight: HashSet<wire::Digest>,
    cursor: Option<wire::Digest>,
    discovering: bool,
    next_scan: Instant,
    phase: Phase,
    drained: Vec<(wire::Digest, ExecutionResult<Progress>)>,
}

impl RevocationSweep {
    pub(crate) fn new(worker: Arc<RevocationWorker>) -> Self {
        Self::with_work(worker)
    }

    pub(super) fn with_work(work: Arc<dyn SweepWork>) -> Self {
        Self {
            activity: None,
            reconciliation: None,
            telemetry: None,
            work,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            queued: VecDeque::new(),
            in_flight: HashSet::new(),
            cursor: None,
            discovering: false,
            next_scan: Instant::now(),
            phase: Phase::Open,
            drained: Vec::new(),
        }
    }

    /// Dropping this wait preserves children, their identities and scan state.
    pub(crate) async fn next(&mut self) -> SweepEvent {
        if !matches!(self.phase, Phase::Open) {
            return SweepEvent::SchedulingStopped;
        }
        self.fill_slots();
        let now = Instant::now();
        let can_discover = !self.discovering
            && self.queued.is_empty()
            && self.in_flight.len() < OPERATION_CONCURRENCY;
        if can_discover && now >= self.next_scan {
            let work = self.work.clone();
            let cursor = self.cursor.clone();
            let activity = self.activity.clone();
            let task = self.tasks.spawn(async move {
                TaskOutput::Discovery(
                    crate::worker_supervision::discover(
                        activity,
                        work.discover(cursor.as_ref()),
                        ExecutionError::Unavailable,
                    )
                    .await,
                )
            });
            self.kinds.insert(task.id(), TaskKind::Discovery);
            self.discovering = true;
            self.next_scan = now + SCAN_INTERVAL;
        }
        let mut wake = now + SCAN_INTERVAL;
        if can_discover && !self.discovering {
            wake = wake.min(self.next_scan);
        }
        tokio::select! {
            biased;
            result = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                self.completed(result.expect("owned task exists"))
            }
            _ = tokio::time::sleep_until(wake) => SweepEvent::Waiting,
        }
    }

    fn fill_slots(&mut self) {
        while self.in_flight.len() < OPERATION_CONCURRENCY {
            let Some(operation) = self.queued.pop_front() else {
                break;
            };
            if !self.in_flight.insert(operation.clone()) {
                continue;
            }
            let work = self.work.clone();
            let original = operation.clone();
            let task = self
                .tasks
                .spawn(async move { TaskOutput::Reconciliation(work.reconcile(&original).await) });
            self.kinds
                .insert(task.id(), TaskKind::Reconciliation(operation));
        }
        if !self.in_flight.is_empty() && self.reconciliation.is_none() {
            self.reconciliation = self
                .activity
                .as_ref()
                .map(|activity| activity.reconciliation());
        }
        self.observe_queue();
    }

    fn observe_queue(&self) {
        if let Some(telemetry) = &self.telemetry {
            telemetry.queue(self.queued.len(), self.in_flight.len());
        }
    }

    fn completed(&mut self, result: Result<(Id, TaskOutput), JoinError>) -> SweepEvent {
        let id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let event = match self
            .kinds
            .remove(&id)
            .expect("every child retains its identity")
        {
            TaskKind::Discovery => {
                self.discovering = false;
                match result {
                    Ok((_, TaskOutput::Discovery(Ok(page)))) => {
                        let count = page.operations.len();
                        let end_of_scan = page.next.is_none();
                        if matches!(self.phase, Phase::Open) {
                            self.cursor = page.next;
                            self.queued = page
                                .operations
                                .into_iter()
                                .filter(|id| !self.in_flight.contains(id))
                                .collect();
                        }
                        SweepEvent::Discovered { count, end_of_scan }
                    }
                    Ok((_, TaskOutput::Discovery(Err(error)))) => {
                        SweepEvent::DiscoveryFailed(error)
                    }
                    _ => SweepEvent::DiscoveryFailed(ExecutionError::Unknown),
                }
            }
            TaskKind::Reconciliation(operation) => {
                self.in_flight.remove(&operation);
                if self.in_flight.is_empty() {
                    self.reconciliation = None;
                }
                let result = match result {
                    Ok((_, TaskOutput::Reconciliation(result))) => result,
                    _ => Err(ExecutionError::Unknown),
                };
                SweepEvent::Reconciled { operation, result }
            }
        };
        self.observe_queue();
        event
    }

    /// Stop local admission, drain owned work, then report aborted IDs as Unknown.
    /// The first deadline and collected results survive cancellation of this wait.
    pub(crate) async fn shutdown(&mut self) -> Vec<(wire::Digest, ExecutionResult<Progress>)> {
        if matches!(self.phase, Phase::Open) {
            self.phase = Phase::Closing {
                deadline: Instant::now() + DRAIN_TIMEOUT,
                aborted: false,
            };
            self.queued.clear();
            self.observe_queue();
        }
        while !self.tasks.is_empty() {
            let Phase::Closing { deadline, aborted } = self.phase else {
                unreachable!()
            };
            let result = if aborted {
                self.tasks.join_next_with_id().await
            } else {
                tokio::select! {
                    biased;
                    result = self.tasks.join_next_with_id() => result,
                    _ = tokio::time::sleep_until(deadline) => {
                        self.tasks.abort_all();
                        self.phase = Phase::Closing { deadline, aborted: true };
                        continue;
                    }
                }
            };
            if let SweepEvent::Reconciled { operation, result } =
                self.completed(result.expect("owned task exists"))
            {
                self.drained.push((operation, result));
            }
        }
        self.phase = Phase::Closed;
        std::mem::take(&mut self.drained)
    }
}
