//! Independently bounded title work discovered from durable, cursor-paged logs.

use layer2_core::diagnostic_context::DiagnosticContext;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use layer2_core::sandbox::store::{Page, PageRequest};
use layer2_core::sandbox::{AgentTitleOutcome, AgentTitleServiceError};
use layer2_core::AgentId;
use tokio::sync::watch;
use tokio::task::{Id, JoinError, JoinHandle, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};
const START_INTERVAL: Duration = Duration::from_secs(10);

use super::title_worker::AgentTitleWorker;

const PAGE_SIZE: u16 = 20;
const CONCURRENCY: usize = 4;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const WORK_TIMEOUT: Duration = Duration::from_secs(35);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

#[async_trait::async_trait]
trait TitleScheduleWork: Send + Sync {
    async fn candidates(&self, page: &PageRequest)
        -> Result<Page<AgentId>, AgentTitleServiceError>;
    async fn run_one(
        self: Arc<Self>,
        id: &AgentId,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError>;
}

#[async_trait::async_trait]
impl TitleScheduleWork for AgentTitleWorker {
    async fn candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleServiceError> {
        DiagnosticContext::background("title_scheduler")
            .with_phase("discovery")
            .scope(self.candidates(page))
            .await
    }

    async fn run_one(
        self: Arc<Self>,
        id: &AgentId,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
        DiagnosticContext::background("title_scheduler")
            .with_phase("execution")
            .scope(AgentTitleWorker::run_one(&self, id))
            .await
    }
}

pub(crate) struct AgentTitleScheduler {
    worker: Arc<dyn TitleScheduleWork>,
    concurrency: usize,
    poll_interval: Duration,
    drain_timeout: Duration,
    start_interval: Duration,
}

impl AgentTitleScheduler {
    pub(super) fn new(worker: Arc<AgentTitleWorker>) -> Self {
        Self {
            worker,
            concurrency: CONCURRENCY,
            poll_interval: POLL_INTERVAL,
            drain_timeout: DRAIN_TIMEOUT,
            start_interval: START_INTERVAL,
        }
    }

    pub(super) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut work = ScheduledTitles::new(self.worker, self.concurrency, self.start_interval);
            let mut ticker = tokio::time::interval(self.poll_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
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
                    "agent title scheduler drain timed out"
                );
                work.tasks.shutdown().await;
            }
        })
    }
}

enum TaskKind {
    Query,
    Agent(AgentId),
}
enum TaskOutput {
    Page(Page<AgentId>),
    Complete,
}
type TaskResult = Result<TaskOutput, &'static str>;

struct ScheduledTitles {
    worker: Arc<dyn TitleScheduleWork>,
    concurrency: usize,
    tasks: JoinSet<TaskResult>,
    kinds: HashMap<Id, TaskKind>,
    in_flight: BTreeSet<AgentId>,
    queued: VecDeque<AgentId>,
    cursor: Option<String>,
    querying: bool,
    start_interval: Duration,
    next_start: Instant,
}

impl ScheduledTitles {
    fn new(
        worker: Arc<dyn TitleScheduleWork>,
        concurrency: usize,
        start_interval: Duration,
    ) -> Self {
        Self {
            worker,
            concurrency,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            in_flight: BTreeSet::new(),
            queued: VecDeque::new(),
            cursor: None,
            querying: false,
            start_interval,
            next_start: Instant::now(),
        }
    }

    fn poll(&mut self) {
        if self.querying || !self.queued.is_empty() || self.in_flight.len() >= self.concurrency {
            return;
        }
        let worker = Arc::clone(&self.worker);
        let page = PageRequest::new(PAGE_SIZE, self.cursor.clone()).expect("bounded title page");
        let task = self.tasks.spawn(async move {
            tokio::time::timeout(QUERY_TIMEOUT, worker.candidates(&page))
                .await
                .map_err(|_| "title discovery timed out")?
                .map(TaskOutput::Page)
                .map_err(|_| "title discovery failed")
        });
        self.kinds.insert(task.id(), TaskKind::Query);
        self.querying = true;
    }

    fn fill_slots(&mut self) {
        while self.in_flight.len() < self.concurrency && Instant::now() >= self.next_start {
            let Some(id) = self.queued.pop_front() else {
                break;
            };
            if !self.in_flight.insert(id.clone()) {
                continue;
            }
            self.next_start = Instant::now() + self.start_interval;
            let worker = Arc::clone(&self.worker);
            let work_id = id.clone();
            let task = self.tasks.spawn(async move {
                tokio::time::timeout(WORK_TIMEOUT, worker.run_one(&work_id))
                    .await
                    .map_err(|_| "title work timed out")?
                    .map(|_| TaskOutput::Complete)
                    .map_err(failure_reason)
            });
            self.kinds.insert(task.id(), TaskKind::Agent(id));
        }
    }

    fn completed(&mut self, result: Result<(Id, TaskResult), JoinError>) {
        let id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        match self.kinds.remove(&id) {
            Some(TaskKind::Query) => self.querying = false,
            Some(TaskKind::Agent(id)) => {
                self.in_flight.remove(&id);
            }
            None => {}
        }
        match result {
            Ok((_, Ok(TaskOutput::Page(page)))) => {
                // Empty eligible pages still advance across the raw Agent collection.
                self.cursor = page.next;
                self.queued = page
                    .items
                    .into_iter()
                    .take(usize::from(PAGE_SIZE))
                    .filter(|id| !self.in_flight.contains(id))
                    .collect();
            }
            Ok((_, Ok(TaskOutput::Complete))) => {}
            Ok((_, Err(reason))) => tracing::warn!(reason, "agent title task deferred"),
            Err(_) => tracing::warn!("agent title task terminated unexpectedly"),
        }
    }
}

/// A bounded, content-free class for the deferral log (#2266).
fn failure_reason(error: AgentTitleServiceError) -> &'static str {
    match error {
        AgentTitleServiceError::InvalidHolder => "title work failed: invalid holder",
        AgentTitleServiceError::InconsistentEvidence => "title work failed: inconsistent evidence",
        AgentTitleServiceError::GenerationUnavailable => {
            "title work failed: generation unavailable"
        }
        AgentTitleServiceError::EffectUnknown => "title work failed: provider effect unresolved",
        AgentTitleServiceError::Budget(_) => "title work failed: budget refused",
        AgentTitleServiceError::Store(_) => "title work failed: store unavailable",
    }
}

#[cfg(test)]
#[path = "title_scheduler_tests.rs"]
mod tests;
