//! Bounded wake-ups over admitted work. Scanning and execution authority are supplied by the service.
use cmaeg_engine::{Disposition, ScheduleConfig, ScheduleReport, WorkSelection};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::{Id, JoinSet},
    time::Instant,
};

pub type Execute<W> =
    Arc<dyn Fn(W) -> Pin<Box<dyn Future<Output = Disposition> + Send>> + Send + Sync>;

pub struct Scheduler<W> {
    execute: Execute<W>,
    config: ScheduleConfig,
    sender: mpsc::Sender<W>,
    receiver: mpsc::Receiver<W>,
    failure: Option<Arc<dyn Fn() + Send + Sync>>,
}
struct Pending<W> {
    items: VecDeque<W>,
    keys: HashSet<(String, String)>,
    cooldown: HashMap<(String, String), Instant>,
}

impl<W> Default for Pending<W> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            keys: HashSet::new(),
            cooldown: HashMap::new(),
        }
    }
}

impl<W: WorkSelection> Scheduler<W> {
    pub fn new(
        execute: Execute<W>,
        config: ScheduleConfig,
    ) -> Result<(Self, mpsc::Sender<W>), &'static str> {
        if !config.valid() {
            return Err("invalid routed scheduler bounds");
        }
        let (sender, receiver) = mpsc::channel(config.queue_capacity);
        let handle = sender.clone();
        Ok((
            Self {
                execute,
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
    pub async fn run<Scan>(
        mut self,
        mut stop: watch::Receiver<bool>,
        scan: impl FnOnce(mpsc::Sender<W>, ScheduleConfig, watch::Receiver<bool>) -> Scan,
    ) -> ScheduleReport
    where
        Scan: Future<Output = ()> + Send + 'static,
    {
        let mut report = ScheduleReport::default();
        if *stop.borrow() {
            return report;
        }
        let mut scan = ScanTask(tokio::spawn(scan(self.sender, self.config, stop.clone())));
        let mut tasks = JoinSet::new();
        let mut active = HashMap::<Id, W>::new();
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
                    tracing::error!(target: "cmaeg::turns::routed::scheduler", "routed recovery task stopped unexpectedly");
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
                            &self.execute,
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
                            &self.execute,
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

fn start_pending<W: WorkSelection>(
    pending: &mut Pending<W>,
    active: &mut HashMap<Id, W>,
    tasks: &mut JoinSet<Disposition>,
    execute: &Execute<W>,
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
        spawn_work(work, active, tasks, execute, report);
    }
    pending.items.extend(deferred);
}

fn spawn_work<W: WorkSelection>(
    work: W,
    active: &mut HashMap<Id, W>,
    tasks: &mut JoinSet<Disposition>,
    execute: &Execute<W>,
    report: &mut ScheduleReport,
) {
    let running = work.clone();
    let task = tasks.spawn(execute(running));
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

pub async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}
