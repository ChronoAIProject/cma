//! Bounded wake-ups over durable admissions; queue contents are never authority.
use crate::ingress::routing_store::{
    AdmissionError, AdmittedPlan, EventIdentity, PlanStore, RecoveryPage,
};
use futures_util::future::BoxFuture;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::{Id, JoinSet},
    time::Instant,
};

/// Executors must use the persisted target and fenced journal before effects.
/// Returning Retry never authorizes another provider POST or platform send.
pub trait TargetExecutor: Send + Sync {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition>;
}

pub trait RecoverySource: Send + Sync {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, AdmissionError>>;
}
impl RecoverySource for PlanStore {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, AdmissionError>> {
        Box::pin(self.recover_all_page(after, 32))
    }
}

#[derive(Clone)]
pub struct WorkItem {
    pub identity: EventIdentity,
    pub target_id: String,
}
impl WorkItem {
    fn key(&self) -> (String, String) {
        (self.identity.admission_id(), self.target_id.clone())
    }
    fn serial_key(&self) -> (String, String) {
        (self.identity.record_id.to_string(), self.target_id.clone())
    }
}

#[derive(Clone, Copy)]
pub enum Disposition {
    /// A durably terminal target, or an ambiguous effect awaiting reconciliation.
    Settled,
    /// Read-only recovery or a proved nonacceptance may be reconsidered later.
    Retry(Duration),
}

#[derive(Clone, Copy)]
pub struct ScheduleConfig {
    pub concurrency: usize,
    pub queue_capacity: usize,
    pub page_delay: Duration,
    pub sweep_delay: Duration,
    pub drain_timeout: Duration,
}
impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            concurrency: 8,
            queue_capacity: 512,
            page_delay: Duration::from_millis(250),
            sweep_delay: Duration::from_secs(5),
            drain_timeout: Duration::from_secs(660),
        }
    }
}
impl ScheduleConfig {
    fn valid(self) -> bool {
        (1..=256).contains(&self.concurrency)
            && (1..=4096).contains(&self.queue_capacity)
            && (Duration::from_millis(10)..=Duration::from_secs(60)).contains(&self.page_delay)
            && (Duration::from_millis(10)..=Duration::from_secs(300)).contains(&self.sweep_delay)
            && (Duration::from_millis(10)..=Duration::from_secs(660)).contains(&self.drain_timeout)
    }
}

#[test]
fn application_drain_fits_the_routed_scheduler_bound() {
    let config = ScheduleConfig {
        drain_timeout: crate::TurnConfig::default().drain,
        ..ScheduleConfig::default()
    };
    assert_eq!(config.drain_timeout, Duration::from_secs(660));
    assert!(config.valid());
    assert!(!ScheduleConfig {
        drain_timeout: Duration::from_secs(661),
        ..config
    }
    .valid());
}

#[derive(Clone)]
pub struct ScheduleHandle {
    sender: mpsc::Sender<WorkItem>,
}
impl ScheduleHandle {
    /// Closed on shutdown or loss of recovery; fresh admissions must pause.
    pub fn accepting(&self) -> bool {
        !self.sender.is_closed()
    }
    /// Only call after admission commits. Full/closed queues do not lose work:
    /// the durable scan will find the original manifest after restart or pressure.
    /// False means this local wake-up did not enqueue every target.
    pub fn notify(&self, plan: &AdmittedPlan) -> bool {
        let mut complete = true;
        for target in &plan.targets {
            if self
                .sender
                .try_send(WorkItem {
                    identity: plan.identity.clone(),
                    target_id: target.id.clone(),
                })
                .is_err()
            {
                complete = false;
            }
        }
        complete
    }
}

pub struct Scheduler {
    source: Arc<dyn RecoverySource>,
    executor: Arc<dyn TargetExecutor>,
    config: ScheduleConfig,
    sender: mpsc::Sender<WorkItem>,
    receiver: mpsc::Receiver<WorkItem>,
    failure: Option<Arc<dyn Fn() + Send + Sync>>,
}
#[derive(Default)]
pub struct ScheduleReport {
    pub started: u64,
    pub finished: u64,
    pub aborted: usize,
    pub recovery_failed: bool,
}

#[derive(Default)]
struct Pending {
    items: VecDeque<WorkItem>,
    keys: HashSet<(String, String)>,
    cooldown: HashMap<(String, String), Instant>,
}

impl Scheduler {
    pub fn new(
        source: Arc<dyn RecoverySource>,
        executor: Arc<dyn TargetExecutor>,
        config: ScheduleConfig,
    ) -> Result<(Self, ScheduleHandle), &'static str> {
        if !config.valid() {
            return Err("invalid routed scheduler bounds");
        }
        let (sender, receiver) = mpsc::channel(config.queue_capacity);
        let handle = ScheduleHandle {
            sender: sender.clone(),
        };
        Ok((
            Self {
                source,
                executor,
                config,
                sender,
                receiver,
                failure: None,
            },
            handle,
        ))
    }

    /// Report loss of recovery immediately, before draining active executions.
    pub fn with_failure_reporter(mut self, report: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.failure = Some(report);
        self
    }

    /// Stop new scheduling immediately, then let admitted tasks finish within
    /// the existing shutdown budget. An aborted effect is recovered by its
    /// durable journal; this scheduler never guesses whether a send happened.
    pub async fn run(mut self, mut stop: watch::Receiver<bool>) -> ScheduleReport {
        let mut report = ScheduleReport::default();
        if *stop.borrow() {
            return report;
        }
        let mut scan = ScanTask(tokio::spawn(scan(
            self.source,
            self.sender,
            self.config,
            stop.clone(),
        )));
        let mut tasks = JoinSet::new();
        let mut active = HashMap::<Id, WorkItem>::new();
        let mut pending = Pending::default();
        let mut scan_finished = false;
        loop {
            tokio::select! {
                biased;
                _ = stopped(&mut stop) => break,
                _ = &mut scan.0 => {
                    report.recovery_failed = true;
                    scan_finished = true;
                    if let Some(failure) = &self.failure { failure(); }
                    tracing::error!("routed recovery task stopped unexpectedly");
                    break;
                }
                joined = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    if let Some(joined) = joined {
                        let (id, disposition) = match joined {
                            Ok((id, value)) => (id, value),
                            Err(error) => (error.id(), Disposition::Retry(self.config.sweep_delay)),
                        };
                        if let Some(work) = active.remove(&id) {
                            remember(
                                &mut pending.cooldown,
                                work.key(),
                                disposition,
                                self.config.queue_capacity,
                            );
                        }
                        report.finished += 1;
                        start_pending(
                            &mut pending,
                            &mut active,
                            &mut tasks,
                            &self.executor,
                            self.config,
                            &mut report,
                        );
                    }
                }
                work = self.receiver.recv(), if tasks.len() < self.config.concurrency => {
                    let Some(work) = work else { break; };
                    let key = work.key();
                    if active.values().any(|item| item.key() == key)
                        || pending.keys.contains(&key)
                        || pending
                            .cooldown
                            .get(&key)
                            .is_some_and(|until| *until > Instant::now())
                    {
                        continue;
                    }
                    if active
                        .values()
                        .any(|item| item.serial_key() == work.serial_key())
                    {
                        if pending.items.len() < self.config.queue_capacity {
                            pending.keys.insert(key);
                            pending.items.push_back(work);
                        }
                    } else {
                        spawn_work(
                            work,
                            &mut active,
                            &mut tasks,
                            &self.executor,
                            &mut report,
                        );
                    }
                }
            }
        }
        self.receiver.close();
        // Scanning has no effects and may be cancelled at any read boundary.
        if !scan_finished {
            scan.0.abort();
            let _ = (&mut scan.0).await;
        }
        let drain = tokio::time::sleep(self.config.drain_timeout);
        tokio::pin!(drain);
        while !tasks.is_empty() {
            tokio::select! {
                _ = &mut drain => {
                    report.aborted = tasks.len();
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                    break;
                }
                result = tasks.join_next() => {
                    if result.is_some() { report.finished += 1; }
                }
            }
        }
        report
    }
}

fn start_pending(
    pending: &mut Pending,
    active: &mut HashMap<Id, WorkItem>,
    tasks: &mut JoinSet<Disposition>,
    executor: &Arc<dyn TargetExecutor>,
    config: ScheduleConfig,
    report: &mut ScheduleReport,
) {
    let mut deferred = VecDeque::new();
    while tasks.len() < config.concurrency {
        let Some(work) = pending.items.pop_front() else {
            break;
        };
        pending.keys.remove(&work.key());
        if pending
            .cooldown
            .get(&work.key())
            .is_some_and(|until| *until > Instant::now())
        {
            continue;
        }
        if active
            .values()
            .any(|item| item.serial_key() == work.serial_key())
        {
            pending.keys.insert(work.key());
            deferred.push_back(work);
            continue;
        }
        spawn_work(work, active, tasks, executor, report);
    }
    pending.items.extend(deferred);
}

fn spawn_work(
    work: WorkItem,
    active: &mut HashMap<Id, WorkItem>,
    tasks: &mut JoinSet<Disposition>,
    executor: &Arc<dyn TargetExecutor>,
    report: &mut ScheduleReport,
) {
    let running = work.clone();
    let executor = executor.clone();
    let task = tasks.spawn(async move { executor.execute(&running).await });
    active.insert(task.id(), work);
    report.started += 1;
}

/// Aborting the outer service must also stop its otherwise detached scanner.
struct ScanTask(tokio::task::JoinHandle<()>);
impl Drop for ScanTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn remember(
    values: &mut HashMap<(String, String), Instant>,
    key: (String, String),
    disposition: Disposition,
    capacity: usize,
) {
    if values.len() >= capacity && !values.contains_key(&key) {
        if let Some(oldest) = values
            .iter()
            .min_by_key(|(_, until)| **until)
            .map(|(key, _)| key.clone())
        {
            values.remove(&oldest);
        }
    }
    let delay = match disposition {
        Disposition::Settled => Duration::from_secs(60),
        Disposition::Retry(delay) => {
            delay.clamp(Duration::from_millis(250), Duration::from_secs(60))
        }
    };
    values.insert(key, Instant::now() + delay);
}

async fn scan(
    source: Arc<dyn RecoverySource>,
    sender: mpsc::Sender<WorkItem>,
    config: ScheduleConfig,
    mut stop: watch::Receiver<bool>,
) {
    let mut cursor = None;
    loop {
        if *stop.borrow() {
            return;
        }
        let page = tokio::select! {
            _ = stopped(&mut stop) => return,
            page = source.page(cursor.as_deref()) => page,
        };
        let delay = match page {
            Ok(page) => {
                if !page.issues.is_empty() {
                    tracing::warn!(
                        issues = page.issues.len(),
                        "routed recovery skipped unreadable admissions"
                    );
                }
                for plan in page.plans {
                    for target in plan.targets {
                        let work = WorkItem {
                            identity: plan.identity.clone(),
                            target_id: target.id,
                        };
                        tokio::select! {
                            _ = stopped(&mut stop) => return,
                            sent = sender.send(work) => if sent.is_err() { return; },
                        }
                    }
                }
                cursor = page.next_cursor;
                if cursor.is_some() {
                    config.page_delay
                } else {
                    config.sweep_delay
                }
            }
            Err(_) => {
                // Preserve the current page after an outage. End-of-scan resets
                // guarantee inserts behind a prior cursor are discovered later.
                tracing::warn!("routed admission scan unavailable; retaining cursor");
                config.sweep_delay
            }
        };
        tokio::select! {
            _ = stopped(&mut stop) => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}
