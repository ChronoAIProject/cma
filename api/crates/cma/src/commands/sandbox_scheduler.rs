use layer2_core::diagnostic_context::DiagnosticContext;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use layer2_core::ids::SandboxCommandId;
use layer2_core::sandbox::control::{SandboxControlError, SandboxControlWorker};
use layer2_core::sandbox::store::{Page, PageRequest};
use tokio::sync::watch;
use tokio::task::{Id, JoinError, JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

const PAGE_SIZE: u16 = 50;

pub(crate) struct SandboxControlScheduler {
    worker: Arc<dyn SandboxControlScheduleWork>,
    interval: Duration,
    concurrency: usize,
    drain_timeout: Duration,
}

#[async_trait::async_trait]
trait SandboxControlScheduleWork: Send + Sync {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxCommandId>, SandboxControlError>;
    async fn run_one(&self, id: &SandboxCommandId) -> Result<(), SandboxControlError>;
}

#[async_trait::async_trait]
impl SandboxControlScheduleWork for SandboxControlWorker {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxCommandId>, SandboxControlError> {
        let page = DiagnosticContext::background("sandbox_control_scheduler")
            .with_phase("discovery")
            .scope(self.discover(page))
            .await?;
        Ok(Page {
            items: page
                .items
                .into_iter()
                .map(|record| record.envelope.command_id)
                .collect(),
            next: page.next,
        })
    }

    async fn run_one(&self, id: &SandboxCommandId) -> Result<(), SandboxControlError> {
        DiagnosticContext::background("sandbox_control_scheduler")
            .with_phase("execution")
            .scope(self.run_one(id))
            .await
    }
}

impl SandboxControlScheduler {
    pub(crate) fn new(worker: Arc<SandboxControlWorker>) -> Self {
        Self {
            worker,
            interval: Duration::from_secs(1),
            concurrency: 8,
            drain_timeout: Duration::from_secs(25),
        }
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut work = ScheduledWork::new(self.worker, self.concurrency);
            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break }
                    }
                    result = work.tasks.join_next_with_id(), if !work.tasks.is_empty() => {
                        if let Some(result) = result { work.completed(result); }
                    }
                    _ = ticker.tick() => work.poll(),
                }
                if !*shutdown.borrow() {
                    work.fill_slots();
                }
            }
            if tokio::time::timeout(self.drain_timeout, async {
                while let Some(result) = work.tasks.join_next_with_id().await {
                    work.completed(result);
                }
            })
            .await
            .is_err()
            {
                tracing::warn!(
                    pending = work.tasks.len(),
                    "Sandbox command scheduler drain timed out"
                );
                work.tasks.shutdown().await;
            }
        })
    }
}

enum TaskKind {
    Discover,
    Command(SandboxCommandId),
}
enum TaskOutput {
    Discovered(Page<SandboxCommandId>),
    Completed,
}
type TaskResult = Result<TaskOutput, SandboxControlError>;

struct ScheduledWork {
    worker: Arc<dyn SandboxControlScheduleWork>,
    concurrency: usize,
    tasks: JoinSet<TaskResult>,
    kinds: HashMap<Id, TaskKind>,
    in_flight: BTreeSet<SandboxCommandId>,
    queued: VecDeque<SandboxCommandId>,
    polling: bool,
    cursor: Option<String>,
}

impl ScheduledWork {
    fn new(worker: Arc<dyn SandboxControlScheduleWork>, concurrency: usize) -> Self {
        Self {
            worker,
            concurrency,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            in_flight: BTreeSet::new(),
            queued: VecDeque::new(),
            polling: false,
            cursor: None,
        }
    }

    fn poll(&mut self) {
        if self.polling || !self.queued.is_empty() || self.in_flight.len() >= self.concurrency {
            return;
        }
        let worker = self.worker.clone();
        let page = PageRequest {
            limit: PAGE_SIZE,
            cursor: self.cursor.clone(),
        };
        let task = self
            .tasks
            .spawn(async move { worker.discover(&page).await.map(TaskOutput::Discovered) });
        self.kinds.insert(task.id(), TaskKind::Discover);
        self.polling = true;
    }

    fn fill_slots(&mut self) {
        while self.in_flight.len() < self.concurrency {
            let Some(id) = self.queued.pop_front() else {
                break;
            };
            if !self.in_flight.insert(id.clone()) {
                continue;
            }
            let worker = self.worker.clone();
            let work_id = id.clone();
            let task = self.tasks.spawn(async move {
                worker
                    .run_one(&work_id)
                    .await
                    .map(|()| TaskOutput::Completed)
            });
            self.kinds.insert(task.id(), TaskKind::Command(id));
        }
    }

    fn completed(&mut self, result: Result<(Id, TaskResult), JoinError>) {
        let id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        match self.kinds.remove(&id) {
            Some(TaskKind::Discover) => self.polling = false,
            Some(TaskKind::Command(id)) => {
                self.in_flight.remove(&id);
            }
            None => {}
        }
        match result {
            Ok((_, Ok(TaskOutput::Discovered(page)))) => {
                // Advance even when all rows remain blocked. Return to the
                // beginning only at the end of a successful scan; IO errors
                // retain the cursor for retry without starving later pages.
                self.cursor = page.next;
                self.queued = page
                    .items
                    .into_iter()
                    .filter(|id| !self.in_flight.contains(id))
                    .collect();
            }
            Ok((_, Ok(TaskOutput::Completed))) => {}
            Ok((_, Err(SandboxControlError::Conflict))) => {}
            Ok((_, Err(error))) => {
                tracing::error!(error = %error, "Sandbox command work failed")
            }
            Err(error) => {
                tracing::error!(error = %error, "Sandbox command task terminated unexpectedly")
            }
        }
    }
}

#[cfg(test)]
#[path = "sandbox_scheduler_tests.rs"]
mod tests;
