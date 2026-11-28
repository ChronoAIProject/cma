use layer2_core::diagnostic_context::DiagnosticContext;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{
    AgentCommandWorker, CommandLedgerError, ScheduledAgentCommand,
};
use layer2_core::sandbox::store::{Page, PageRequest};
use layer2_core::AgentId;
use tokio::sync::{broadcast, watch, Notify};
use tokio::task::{Id, JoinError, JoinHandle, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};

const PAGE_SIZE: u16 = 50;
// At idle, at most four indexed discovery scans per second per replica. Existing
// single-discovery, pagination and eight-command backpressure remain bounded.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const CANCEL_WAKE_SPACING: Duration = Duration::from_millis(50);
pub(super) const MAX_DEFERRED: usize = 4_096;

pub(crate) struct CommandScheduler {
    worker: Arc<dyn CommandScheduleWork>,
    interval: Duration,
    concurrency: usize,
    drain_timeout: Duration,
    reconcile_pacing: bool,
    cancel_wake: Option<Arc<Notify>>,
    turn_terminal: Option<broadcast::Sender<AgentId>>,
}

#[async_trait::async_trait]
trait CommandScheduleWork: Send + Sync {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<ScheduledAgentCommand>, CommandLedgerError>;
    async fn run_one(&self, id: &AgentCommandId) -> Result<Duration, CommandLedgerError>;
}

#[async_trait::async_trait]
impl CommandScheduleWork for AgentCommandWorker {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<ScheduledAgentCommand>, CommandLedgerError> {
        let discovery = DiagnosticContext::background("command_scheduler")
            .with_phase("discovery")
            .scope(self.discover_scheduled(page))
            .await?;
        if discovery.heads_failed {
            crate::telemetry::command_reconcile::heads_failed();
        }
        Ok(discovery.page)
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
            reconcile_pacing: false,
            cancel_wake: None,
            turn_terminal: None,
        }
    }

    pub(crate) fn with_reconcile_pacing(mut self, enabled: bool) -> Self {
        self.reconcile_pacing = enabled;
        self
    }

    pub(crate) fn with_cancel_wake(mut self, wake: Option<Arc<Notify>>) -> Self {
        self.cancel_wake = wake;
        self
    }

    pub(crate) fn with_turn_terminal_wake(mut self, wake: broadcast::Sender<AgentId>) -> Self {
        self.turn_terminal = Some(wake);
        self
    }

    #[cfg(test)]
    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        self.spawn_supervised(
            shutdown,
            crate::worker_supervision::Workers::new().get("agent_commands"),
        )
    }

    pub(crate) fn spawn_supervised(
        self,
        shutdown: watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> JoinHandle<()> {
        let interval = self.interval;
        let drain_timeout = self.drain_timeout;
        let cancel_wake = self.cancel_wake;
        let turn_terminal = self.turn_terminal;
        let work = ScheduledWork::new(self.worker, self.concurrency, self.reconcile_pacing);
        crate::worker_supervision::spawn(
            work,
            activity,
            shutdown,
            move |work, activity, mut shutdown| {
                let cancel_wake = cancel_wake.clone();
                let mut turn_terminal = turn_terminal.as_ref().map(broadcast::Sender::subscribe);
                Box::pin(async move {
                    work.activity = Some(activity.clone());
                    let telemetry = crate::telemetry::worker::Worker::start("agent_commands");
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
                        let next_cancel_scan = work.next_cancel_scan;
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() { break }
                            }
                            result = work.tasks.join_next_with_id(), if !work.tasks.is_empty() => {
                                if let Some(result) = result { work.completed(result); }
                            }
                            _ = ticker.tick() => work.poll(),
                            agent = async {
                                if let Some(wake) = &mut turn_terminal {
                                    wake.recv().await.ok()
                                } else {
                                    std::future::pending().await
                                }
                            } => {
                                if let Some(agent) = agent {
                                    work.turn_terminal(&agent);
                                }
                            }
                            _ = async {
                                tokio::time::sleep_until(next_cancel_scan).await;
                                if let Some(wake) = &cancel_wake {
                                    wake.notified().await;
                                }
                            }, if cancel_wake.is_some() && work.can_poll() => {
                                if !*shutdown.borrow() {
                                    work.poll();
                                    work.next_cancel_scan = Instant::now() + CANCEL_WAKE_SPACING;
                                }
                            }
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
                            "Agent command scheduler drain timed out"
                        );
                        work.tasks.shutdown().await;
                    }
                })
            },
        )
    }
}

enum TaskKind {
    Discover,
    Command(ScheduledAgentCommand, bool),
}
enum TaskOutput {
    Discovered(Page<ScheduledAgentCommand>),
    Completed(Duration),
}
type TaskResult = Result<TaskOutput, CommandLedgerError>;

struct ScheduledWork {
    activity: Option<crate::worker_supervision::Activity>,
    worker: Arc<dyn CommandScheduleWork>,
    concurrency: usize,
    tasks: JoinSet<TaskResult>,
    kinds: HashMap<Id, TaskKind>,
    in_flight: BTreeSet<AgentCommandId>,
    not_before: HashMap<AgentCommandId, (Instant, Option<AgentId>)>,
    queued: VecDeque<ScheduledAgentCommand>,
    reconciliation: super::reconcile_schedule::ReconcileSchedule,
    polling: bool,
    cursor: Option<String>,
    next_cancel_scan: Instant,
}

impl ScheduledWork {
    fn new(
        worker: Arc<dyn CommandScheduleWork>,
        concurrency: usize,
        reconcile_pacing: bool,
    ) -> Self {
        Self {
            activity: None,
            worker,
            concurrency,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            in_flight: BTreeSet::new(),
            not_before: HashMap::new(),
            queued: VecDeque::new(),
            reconciliation: super::reconcile_schedule::ReconcileSchedule::new(reconcile_pacing),
            polling: false,
            cursor: None,
            next_cancel_scan: Instant::now(),
        }
    }

    fn can_poll(&self) -> bool {
        !self.polling && self.queued.is_empty() && self.in_flight.len() < self.concurrency
    }

    fn poll(&mut self) {
        let now = Instant::now();
        self.not_before.retain(|_, (deadline, _)| *deadline > now);
        self.reconciliation.prune(now);
        if !self.can_poll() {
            return;
        }
        let worker = self.worker.clone();
        let page = PageRequest {
            limit: PAGE_SIZE,
            cursor: self.cursor.clone(),
        };
        let activity = self.activity.clone();
        let task = self.tasks.spawn(async move {
            crate::worker_supervision::discover(
                activity,
                worker.discover(&page),
                CommandLedgerError::Unavailable,
            )
            .await
            .map(TaskOutput::Discovered)
        });
        self.kinds.insert(task.id(), TaskKind::Discover);
        self.polling = true;
    }

    fn turn_terminal(&mut self, agent: &AgentId) {
        self.not_before
            .retain(|_, (_, waiting_agent)| waiting_agent.as_ref() != Some(agent));
        for kind in self.kinds.values_mut() {
            if let TaskKind::Command(item, woken) = kind {
                if item.turn_wait_agent.as_ref() == Some(agent) {
                    *woken = true;
                }
            }
        }
    }

    fn fill_slots(&mut self) {
        while self.in_flight.len() < self.concurrency {
            let Some(item) = self.queued.pop_front() else {
                break;
            };
            let id = &item.id;
            if self
                .not_before
                .get(id)
                .is_some_and(|(deadline, _)| *deadline > Instant::now())
                || !self.in_flight.insert(id.clone())
            {
                continue;
            }
            let worker = self.worker.clone();
            let work_id = id.clone();
            let task = self
                .tasks
                .spawn(async move { worker.run_one(&work_id).await.map(TaskOutput::Completed) });
            self.kinds.insert(task.id(), TaskKind::Command(item, false));
        }
    }

    fn completed(&mut self, result: Result<(Id, TaskResult), JoinError>) {
        let id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let phase = if matches!(self.kinds.get(&id), Some(TaskKind::Discover)) {
            "discovery"
        } else {
            "execution"
        };
        let outcome = match &result {
            Ok((_, Ok(_))) => "ok",
            Ok((_, Err(_))) => "error",
            Err(_) => "terminated",
        };
        crate::telemetry::worker::completed("agent_commands", phase, outcome);
        match self.kinds.remove(&id) {
            Some(TaskKind::Discover) => self.polling = false,
            Some(TaskKind::Command(item, woken)) => {
                let id = item.id;
                self.in_flight.remove(&id);
                let now = Instant::now();
                let delay = match &result {
                    Ok((_, Ok(TaskOutput::Completed(delay)))) => {
                        if item.dispatched {
                            self.reconciliation.succeeded(&id, now);
                        }
                        *delay
                    }
                    _ if item.dispatched => {
                        crate::telemetry::command_reconcile::Outcome::Error.record();
                        let code = match &result {
                            Ok((_, Err(error))) => error.code(),
                            _ => "task_terminated",
                        };
                        tracing::warn!(
                            code,
                            "Dispatched command reconciliation failed; scheduling retry"
                        );
                        self.reconciliation.failed(&id, now)
                    }
                    _ => Duration::ZERO,
                };
                if !woken && !delay.is_zero() && self.not_before.len() < MAX_DEFERRED {
                    // Backoff belongs to discovery, not a scarce execution slot.
                    self.not_before
                        .insert(id, (now + delay, item.turn_wait_agent));
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
                    .filter(|item| {
                        !self.in_flight.contains(&item.id)
                            && !self.not_before.contains_key(&item.id)
                    })
                    .filter(|item| {
                        let skip = item.dispatched
                            && item.reconcile_current
                            && self.reconciliation.current(&item.id, Instant::now());
                        if skip {
                            crate::telemetry::command_reconcile::Outcome::SkippedCurrent.record();
                        }
                        !skip
                    })
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
                tracing::error!(
                    cancelled = error.is_cancelled(),
                    panicked = error.is_panic(),
                    "Agent command task terminated unexpectedly"
                )
            }
        }
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
