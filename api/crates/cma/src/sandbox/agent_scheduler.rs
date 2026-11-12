//! Bounded Agent work scheduling without a sweep-wide authentication barrier.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::AgentId;
use tokio::sync::{watch, Notify};
use tokio::task::{Id, JoinError, JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use super::agent_worker::AgentWorker;

const SWEEP_INTERVAL: Duration = Duration::from_secs(1);
const RECONCILIATION_FALLBACK_INTERVAL: Duration = Duration::from_secs(30);
const MAX_CONCURRENT_AGENTS: usize = 50;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

pub(crate) struct AgentScheduler {
    budget: Arc<crate::background::BackgroundBudget>,
    worker: Arc<dyn AgentScheduleWork>,
    sweep_interval: Duration,
    concurrency: usize,
    drain_timeout: Duration,
    admitted: Arc<Notify>,
    parent_ready: Arc<Notify>,
    event_driven_reconciliation: bool,
}

#[async_trait::async_trait]
trait AgentScheduleWork: Send + Sync {
    async fn due(&self, now: SystemTime, limit: u16) -> Result<Vec<AgentId>, SandboxStoreError>;
    async fn reconcile(&self, now: SystemTime) -> Result<usize, SandboxStoreError>;
    async fn run_one(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError>;
}

#[async_trait::async_trait]
impl AgentScheduleWork for AgentWorker {
    async fn due(&self, now: SystemTime, limit: u16) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.due_work(now, limit).await
    }

    async fn reconcile(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        self.reconcile_connections(now).await
    }

    async fn run_one(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError> {
        AgentWorker::run_scheduled(self, id, now).await
    }
}

impl AgentScheduler {
    pub(crate) fn new(worker: Arc<AgentWorker>, admitted: Arc<Notify>) -> Self {
        Self {
            budget: crate::background::shared(),
            worker,
            sweep_interval: SWEEP_INTERVAL,
            concurrency: MAX_CONCURRENT_AGENTS,
            drain_timeout: DRAIN_TIMEOUT,
            admitted,
            parent_ready: Arc::new(Notify::new()),
            event_driven_reconciliation: false,
        }
    }

    pub(crate) fn with_event_driven_reconciliation(
        mut self,
        parent_ready: Arc<Notify>,
        enabled: bool,
    ) -> Self {
        self.parent_ready = parent_ready;
        self.event_driven_reconciliation = enabled;
        self
    }

    #[cfg(test)]
    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        self.spawn_supervised(
            shutdown,
            crate::worker_supervision::Workers::new().get("agents"),
        )
    }

    pub(crate) fn spawn_supervised(
        self,
        shutdown: watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> JoinHandle<()> {
        let interval = self.sweep_interval;
        let drain_timeout = self.drain_timeout;
        let admitted = self.admitted;
        let parent_ready = self.parent_ready;
        let event_driven_reconciliation = self.event_driven_reconciliation;
        let work = ScheduledWork::new(self.worker, self.concurrency, self.budget);
        crate::worker_supervision::spawn(
            work,
            activity,
            shutdown,
            move |work, activity, mut shutdown| {
                let admitted = admitted.clone();
                let parent_ready = parent_ready.clone();
                Box::pin(async move {
                    work.activity = Some(activity.clone());
                    let telemetry = crate::telemetry::worker::Worker::start("agents");
                    let mut ticker = tokio::time::interval(interval);
                    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    let mut reconciliation_ticker =
                        tokio::time::interval(RECONCILIATION_FALLBACK_INTERVAL);
                    reconciliation_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        activity.waiting(
                            interval,
                            if work.in_flight.len() >= work.concurrency {
                                "execution_slots"
                            } else if work.polling {
                                "discovery"
                            } else {
                                "idle_scan"
                            },
                        );
                        telemetry.heartbeat();
                        telemetry.queue(work.queued.len(), work.in_flight.len());
                        if *shutdown.borrow() {
                            break;
                        }
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() {
                                    break;
                                }
                            }
                            result = work.tasks.join_next_with_id(), if !work.tasks.is_empty() => {
                                if let Some(result) = result {
                                    work.completed(result);
                                }
                            }
                            _ = ticker.tick() => work.poll(!event_driven_reconciliation),
                            _ = reconciliation_ticker.tick(), if event_driven_reconciliation => {
                                work.reconcile();
                            }
                            _ = parent_ready.notified(), if event_driven_reconciliation => {
                                work.reconcile();
                            }
                            // Keep one coalesced hint through a full queue, full capacity,
                            // or an in-progress discovery that may predate the admission.
                            _ = admitted.notified(), if work.can_discover() => work.discover(),
                        }
                        if !*shutdown.borrow() {
                            work.fill_slots();
                        }
                        activity.scheduled();
                    }
                })
            },
            move |work| {
                Box::pin(async move {
                    if tokio::time::timeout(drain_timeout, async {
                        while let Some(result) = work.tasks.join_next_with_id().await {
                            work.completed(result);
                        }
                    })
                    .await
                    .is_err()
                    {
                        tracing::warn!(
                            pending = work.tasks.len(),
                            "agent scheduler drain timed out"
                        );
                        work.tasks.shutdown().await;
                    }
                })
            },
        )
    }
}

enum TaskKind {
    Due,
    Reconcile,
    Agent(AgentId),
}

enum TaskOutput {
    Due(Vec<AgentId>),
    Completed(usize),
}

type TaskResult = Result<TaskOutput, SandboxStoreError>;

struct ScheduledWork {
    activity: Option<crate::worker_supervision::Activity>,
    reconciliation: crate::background::BackgroundWork,
    worker: Arc<dyn AgentScheduleWork>,
    concurrency: usize,
    tasks: JoinSet<TaskResult>,
    kinds: HashMap<Id, TaskKind>,
    in_flight: BTreeSet<AgentId>,
    queued: VecDeque<AgentId>,
    polling: bool,
    reconciling: bool,
}

impl ScheduledWork {
    fn new(
        worker: Arc<dyn AgentScheduleWork>,
        concurrency: usize,
        budget: Arc<crate::background::BackgroundBudget>,
    ) -> Self {
        Self {
            activity: None,
            reconciliation: crate::background::BackgroundWork::new(
                budget,
                "agent_reconciliation",
                Duration::from_secs(30),
                SWEEP_INTERVAL,
            ),
            worker,
            concurrency,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            in_flight: BTreeSet::new(),
            queued: VecDeque::new(),
            polling: false,
            reconciling: false,
        }
    }

    fn poll(&mut self, reconcile: bool) {
        if reconcile {
            self.reconcile();
        }
        self.discover();
    }

    fn reconcile(&mut self) {
        if !self.reconciling {
            let worker = Arc::clone(&self.worker);
            let reconciliation = self.reconciliation.clone();
            let activity = self.activity.clone();
            let task = self.tasks.spawn(async move {
                reconciliation
                    .run(|| async {
                        let _phase = activity.map(|activity| activity.reconciliation());
                        worker.reconcile(SystemTime::now()).await
                    })
                    .await
                    .map(|completed| TaskOutput::Completed(completed.unwrap_or(0)))
            });
            self.kinds.insert(task.id(), TaskKind::Reconcile);
            self.reconciling = true;
        }
    }

    fn can_discover(&self) -> bool {
        !self.polling && self.queued.is_empty() && self.in_flight.len() < self.concurrency
    }

    fn discover(&mut self) {
        if self.can_discover() {
            let worker = Arc::clone(&self.worker);
            let activity = self.activity.clone();
            let task = self.tasks.spawn(async move {
                crate::worker_supervision::discover(
                    activity,
                    worker.due(SystemTime::now(), MAX_PAGE_SIZE),
                    SandboxStoreError::Unavailable("discovery deadline".into()),
                )
                .await
                .map(TaskOutput::Due)
            });
            self.kinds.insert(task.id(), TaskKind::Due);
            self.polling = true;
        }
    }

    fn fill_slots(&mut self) {
        while self.in_flight.len() < self.concurrency {
            let Some(id) = self.queued.pop_front() else {
                break;
            };
            if !self.in_flight.insert(id.clone()) {
                continue;
            }
            let worker = Arc::clone(&self.worker);
            let priority = self.reconciliation.clone();
            let work_id = id.clone();
            let task = self.tasks.spawn(async move {
                let _priority = priority.foreground();
                // The query and local queue confer no authority; claim using fresh time.
                worker
                    .run_one(&work_id, SystemTime::now())
                    .await
                    .map(|completed| TaskOutput::Completed(usize::from(completed)))
            });
            self.kinds.insert(task.id(), TaskKind::Agent(id));
        }
    }

    fn completed(&mut self, result: Result<(Id, TaskResult), JoinError>) {
        let task_id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let phase = match self.kinds.get(&task_id) {
            Some(TaskKind::Due) => "discovery",
            Some(TaskKind::Reconcile) => "reconciliation",
            _ => "execution",
        };
        let outcome = match &result {
            Ok((_, Ok(_))) => "ok",
            Ok((_, Err(_))) => "error",
            Err(_) => "terminated",
        };
        crate::telemetry::worker::completed("agents", phase, outcome);
        match self.kinds.remove(&task_id) {
            Some(TaskKind::Due) => self.polling = false,
            Some(TaskKind::Reconcile) => self.reconciling = false,
            Some(TaskKind::Agent(id)) => {
                self.in_flight.remove(&id);
            }
            None => {}
        }
        match result {
            Ok((_, Ok(TaskOutput::Due(ids)))) => {
                self.queued = ids
                    .into_iter()
                    .take(usize::from(MAX_PAGE_SIZE))
                    .filter(|id| !self.in_flight.contains(id))
                    .collect();
            }
            Ok((_, Ok(TaskOutput::Completed(completed)))) if completed > 0 => {
                tracing::debug!(completed, "agent lifecycle work completed");
            }
            Ok((_, Ok(_))) => {}
            Ok((_, Err(error))) => {
                tracing::error!(error = %error, "agent lifecycle task failed");
            }
            Err(error) => {
                tracing::error!(error = %error, "agent lifecycle task terminated unexpectedly");
            }
        }
    }
}

#[cfg(test)]
#[path = "agent_scheduler_tests.rs"]
mod tests;
