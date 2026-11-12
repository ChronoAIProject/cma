use layer2_core::diagnostic_context::DiagnosticContext;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{AgentCommandWorker, CommandLedgerError};
use layer2_core::sandbox::store::{Page, PageRequest};
use tokio::sync::watch;
use tokio::task::{Id, JoinError, JoinHandle, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};

const PAGE_SIZE: u16 = 50;
// At idle, at most four indexed discovery scans per second per replica. Existing
// single-discovery, pagination and eight-command backpressure remain bounded.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_DEFERRED: usize = 4_096;

pub(crate) struct CommandScheduler {
    worker: Arc<dyn CommandScheduleWork>,
    interval: Duration,
    concurrency: usize,
    drain_timeout: Duration,
}

#[async_trait::async_trait]
trait CommandScheduleWork: Send + Sync {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentCommandId>, CommandLedgerError>;
    async fn run_one(&self, id: &AgentCommandId) -> Result<Duration, CommandLedgerError>;
}

#[async_trait::async_trait]
impl CommandScheduleWork for AgentCommandWorker {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentCommandId>, CommandLedgerError> {
        let page = DiagnosticContext::background("command_scheduler")
            .with_phase("discovery")
            .scope(self.discover(page))
            .await?;
        Ok(Page {
            items: page
                .items
                .into_iter()
                .map(|record| record.receipt.envelope.command_id)
                .collect(),
            next: page.next,
        })
    }

    async fn run_one(&self, id: &AgentCommandId) -> Result<Duration, CommandLedgerError> {
        DiagnosticContext::background("command_scheduler")
            .with_phase("execution")
            .scope(self.run_scheduled(id))
            .await
    }
}

impl CommandScheduler {
    pub(crate) fn new(worker: Arc<AgentCommandWorker>) -> Self {
        Self {
            worker,
            interval: POLL_INTERVAL,
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
                    "Agent command scheduler drain timed out"
                );
                work.tasks.shutdown().await;
            }
        })
    }
}

enum TaskKind {
    Discover,
    Command(AgentCommandId),
}
enum TaskOutput {
    Discovered(Page<AgentCommandId>),
    Completed(Duration),
}
type TaskResult = Result<TaskOutput, CommandLedgerError>;

struct ScheduledWork {
    worker: Arc<dyn CommandScheduleWork>,
    concurrency: usize,
    tasks: JoinSet<TaskResult>,
    kinds: HashMap<Id, TaskKind>,
    in_flight: BTreeSet<AgentCommandId>,
    not_before: HashMap<AgentCommandId, Instant>,
    queued: VecDeque<AgentCommandId>,
    polling: bool,
    cursor: Option<String>,
}

impl ScheduledWork {
    fn new(worker: Arc<dyn CommandScheduleWork>, concurrency: usize) -> Self {
        Self {
            worker,
            concurrency,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            in_flight: BTreeSet::new(),
            not_before: HashMap::new(),
            queued: VecDeque::new(),
            polling: false,
            cursor: None,
        }
    }

    fn poll(&mut self) {
        let now = Instant::now();
        self.not_before.retain(|_, deadline| *deadline > now);
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
            if self
                .not_before
                .get(&id)
                .is_some_and(|deadline| *deadline > Instant::now())
                || !self.in_flight.insert(id.clone())
            {
                continue;
            }
            let worker = self.worker.clone();
            let work_id = id.clone();
            let task = self
                .tasks
                .spawn(async move { worker.run_one(&work_id).await.map(TaskOutput::Completed) });
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
                if let Ok((_, Ok(TaskOutput::Completed(delay)))) = &result {
                    if !delay.is_zero() && self.not_before.len() < MAX_DEFERRED {
                        // Backoff belongs to discovery, not a scarce execution slot.
                        // Expired/retired entries leave on the next bounded poll.
                        self.not_before.insert(id, Instant::now() + *delay);
                    }
                }
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
                    .filter(|id| !self.in_flight.contains(id) && !self.not_before.contains_key(id))
                    .collect();
            }
            Ok((_, Ok(TaskOutput::Completed(_)))) => {}
            Ok((_, Err(CommandLedgerError::StaleClaim | CommandLedgerError::Conflict))) => {}
            Ok((_, Err(error))) => {
                tracing::error!(
                    target: "cma::bridge::operations",
                    operation = "command_worker_failure", code = error.code(),
                    "Agent command work failed"
                )
            }
            Err(error) => {
                tracing::error!(error = %error, "Agent command task terminated unexpectedly")
            }
        }
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
