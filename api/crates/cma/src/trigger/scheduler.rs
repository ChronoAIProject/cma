//! Independent, bounded Launch scheduling: a slow dependency must not delay other visitors.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::LaunchId;
use tokio::sync::watch;
use tokio::task::{Id, JoinError, JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use super::worker::{LaunchWorker, LaunchWorkerError};

const SWEEP_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CONCURRENT_LAUNCHES: usize = 8;
const WORK_BATCH_SIZE: u16 = 50;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

pub(crate) struct LaunchScheduler {
    worker: Arc<dyn LaunchScheduleWork>,
    sweep_interval: Duration,
    concurrency: usize,
    drain_timeout: Duration,
}

#[async_trait::async_trait]
trait LaunchScheduleWork: Send + Sync {
    async fn due(&self, now: SystemTime) -> Result<Vec<LaunchId>, LaunchWorkerError>;
    async fn run_one(&self, id: &LaunchId, now: SystemTime) -> Result<bool, LaunchWorkerError>;
}

#[async_trait::async_trait]
impl LaunchScheduleWork for LaunchWorker {
    async fn due(&self, now: SystemTime) -> Result<Vec<LaunchId>, LaunchWorkerError> {
        self.due_work(now, WORK_BATCH_SIZE).await
    }

    async fn run_one(&self, id: &LaunchId, now: SystemTime) -> Result<bool, LaunchWorkerError> {
        self.run_scheduled(id, now).await
    }
}

impl LaunchScheduler {
    pub(crate) fn new(worker: Arc<LaunchWorker>) -> Self {
        Self {
            worker,
            sweep_interval: SWEEP_INTERVAL,
            concurrency: MAX_CONCURRENT_LAUNCHES,
            drain_timeout: DRAIN_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        self.spawn_supervised(
            shutdown,
            crate::worker_supervision::Workers::new().get("launches"),
        )
    }

    pub(crate) fn spawn_supervised(
        self,
        shutdown: watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> JoinHandle<()> {
        let interval = self.sweep_interval;
        let drain_timeout = self.drain_timeout;
        let work = ScheduledWork::new(self.worker, self.concurrency);
        crate::worker_supervision::spawn(
            work,
            activity,
            shutdown,
            move |work, activity, mut shutdown| {
                Box::pin(async move {
                    work.activity = Some(activity.clone());
                    let telemetry = crate::telemetry::worker::Worker::start("launches");
                    let mut ticker = tokio::time::interval(interval);
                    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
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
                            _ = ticker.tick() => work.poll(),
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
                            "Launch scheduler drain timed out"
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
    Launch(LaunchId),
}

enum TaskOutput {
    Due(Vec<LaunchId>),
    Completed(bool),
}

type TaskResult = Result<TaskOutput, LaunchWorkerError>;

struct ScheduledWork {
    activity: Option<crate::worker_supervision::Activity>,
    worker: Arc<dyn LaunchScheduleWork>,
    concurrency: usize,
    tasks: JoinSet<TaskResult>,
    kinds: HashMap<Id, TaskKind>,
    in_flight: BTreeSet<LaunchId>,
    queued: VecDeque<LaunchId>,
    polling: bool,
}

impl ScheduledWork {
    fn new(worker: Arc<dyn LaunchScheduleWork>, concurrency: usize) -> Self {
        Self {
            activity: None,
            worker,
            concurrency,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            in_flight: BTreeSet::new(),
            queued: VecDeque::new(),
            polling: false,
        }
    }

    fn poll(&mut self) {
        if !self.polling && self.queued.is_empty() && self.in_flight.len() < self.concurrency {
            let worker = Arc::clone(&self.worker);
            let activity = self.activity.clone();
            let task = self.tasks.spawn(async move {
                crate::worker_supervision::discover(
                    activity,
                    worker.due(SystemTime::now()),
                    LaunchWorkerError::Store(
                        layer2_core::trigger_domain::LaunchStoreError::Unavailable(
                            "discovery deadline".into(),
                        ),
                    ),
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
            let work_id = id.clone();
            let task = self.tasks.spawn(async move {
                // Discovery grants no authority; every step still takes a fresh durable claim.
                worker
                    .run_one(&work_id, SystemTime::now())
                    .await
                    .map(TaskOutput::Completed)
            });
            self.kinds.insert(task.id(), TaskKind::Launch(id));
        }
    }

    fn completed(&mut self, result: Result<(Id, TaskResult), JoinError>) {
        let id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let phase = if matches!(self.kinds.get(&id), Some(TaskKind::Due)) {
            "discovery"
        } else {
            "execution"
        };
        let outcome = match &result {
            Ok((_, Ok(_))) => "ok",
            Ok((_, Err(_))) => "error",
            Err(_) => "terminated",
        };
        crate::telemetry::worker::completed("launches", phase, outcome);
        let task_id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let launch_id = match self.kinds.remove(&task_id) {
            Some(TaskKind::Due) => {
                self.polling = false;
                None
            }
            Some(TaskKind::Launch(id)) => {
                self.in_flight.remove(&id);
                Some(id)
            }
            None => None,
        };
        match result {
            Ok((_, Ok(TaskOutput::Due(ids)))) => {
                self.queued = ids
                    .into_iter()
                    .take(usize::from(WORK_BATCH_SIZE))
                    .filter(|id| !self.in_flight.contains(id))
                    .collect();
            }
            Ok((_, Ok(TaskOutput::Completed(true)))) => {
                tracing::debug!(?launch_id, "Trigger Launch work completed");
            }
            Ok((_, Ok(_))) => {}
            Ok((_, Err(error))) => {
                tracing::error!(?launch_id, error = %error, "Trigger Launch task failed");
            }
            Err(error) => {
                tracing::error!(?launch_id, error = %error, "Trigger Launch task terminated unexpectedly");
            }
        }
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
