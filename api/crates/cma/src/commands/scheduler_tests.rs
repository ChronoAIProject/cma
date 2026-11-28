use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use tokio::sync::Notify;

use super::*;

pub(crate) struct DrivenScheduler {
    work: ScheduledWork,
    interval: Duration,
}

impl CommandScheduler {
    pub(crate) fn driven(self) -> DrivenScheduler {
        DrivenScheduler {
            work: ScheduledWork::new(self.worker, self.concurrency, self.reconcile_pacing),
            interval: self.interval,
        }
    }

    pub(crate) fn observe_discovery(
        mut self,
        observations: tokio::sync::mpsc::Sender<Vec<AgentCommandId>>,
        first_scan_release: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Self {
        self.worker = Arc::new(ObservedWork {
            inner: self.worker,
            observations,
            first_scan_release: Mutex::new(first_scan_release),
        });
        self
    }
}

struct ObservedWork {
    inner: Arc<dyn CommandScheduleWork>,
    observations: tokio::sync::mpsc::Sender<Vec<AgentCommandId>>,
    first_scan_release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait::async_trait]
impl CommandScheduleWork for ObservedWork {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<ScheduledAgentCommand>, CommandLedgerError> {
        let result = self.inner.discover(page).await?;
        let _ = self
            .observations
            .try_send(result.items.iter().map(|item| item.id.clone()).collect());
        let release = self.first_scan_release.lock().unwrap().take();
        if let Some(release) = release {
            release.await.expect("first discovery release");
        }
        Ok(result)
    }

    async fn run_one(&self, id: &AgentCommandId) -> Result<Duration, CommandLedgerError> {
        self.inner.run_one(id).await
    }
}

impl DrivenScheduler {
    pub(crate) async fn poll(&mut self) {
        // Keep real cooldowns and discovery cadence, but finish each poll's
        // work before the next one so slow Mongo scans cannot lose attempts.
        tokio::time::sleep(self.interval).await;
        self.work.poll();
        while let Some(result) = self.work.tasks.join_next_with_id().await {
            match &result {
                Err(error) => panic!("driven scheduler task failed: {error}"),
                Ok((_, Err(error))) => panic!("driven scheduler work failed: {error:?}"),
                Ok((_, Ok(_))) => {}
            }
            self.work.completed(result);
            self.work.fill_slots();
        }
        assert!(self.work.in_flight.is_empty());
        assert!(self.work.queued.is_empty());
        assert!(!self.work.polling);
    }
}

#[derive(Clone)]
enum Behavior {
    Complete,
    Blocked,
    Deferred(Duration),
    Wait(Arc<Notify>),
    Fail,
    Conflict,
    Stale,
    Panic,
}

#[derive(Default)]
struct Probe {
    dispatched: Mutex<BTreeMap<AgentCommandId, bool>>,
    turn_waits: Mutex<BTreeMap<AgentCommandId, AgentId>>,
    jobs: Mutex<BTreeMap<AgentCommandId, Behavior>>,
    calls: Mutex<Vec<AgentCommandId>>,
    discovery_calls: Mutex<Vec<Instant>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    fail_poll: AtomicBool,
}

impl Probe {
    fn put(&self, name: &str, behavior: Behavior) -> AgentCommandId {
        let id = AgentCommandId(name.into());
        self.jobs.lock().unwrap().insert(id.clone(), behavior);
        id
    }

    fn count(&self, id: &AgentCommandId) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|called| *called == id)
            .count()
    }

    fn scheduler(self: &Arc<Self>, concurrency: usize) -> CommandScheduler {
        CommandScheduler {
            worker: self.clone(),
            interval: POLL_INTERVAL,
            concurrency,
            drain_timeout: Duration::from_secs(5),
            reconcile_pacing: false,
            cancel_wake: None,
            turn_terminal: None,
        }
    }
}

struct ActiveCall<'a>(&'a Probe);

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl CommandScheduleWork for Probe {
    async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<ScheduledAgentCommand>, CommandLedgerError> {
        self.discovery_calls.lock().unwrap().push(Instant::now());
        if self.fail_poll.swap(false, Ordering::SeqCst) {
            return Err(CommandLedgerError::Unavailable);
        }
        let jobs = self.jobs.lock().unwrap();
        let mut ids: Vec<_> = jobs
            .keys()
            .filter(|id| {
                page.cursor
                    .as_deref()
                    .is_none_or(|cursor| id.as_str() > cursor)
            })
            .take(usize::from(page.limit) + 1)
            .cloned()
            .collect();
        let next = (ids.len() > usize::from(page.limit))
            .then(|| ids[usize::from(page.limit) - 1].to_string());
        ids.truncate(usize::from(page.limit));
        Ok(Page {
            items: ids
                .into_iter()
                .map(|id| {
                    let dispatched = self.dispatched.lock().unwrap();
                    ScheduledAgentCommand {
                        turn_wait_agent: self.turn_waits.lock().unwrap().get(&id).cloned(),
                        dispatched: dispatched.contains_key(&id),
                        reconcile_current: dispatched.get(&id).copied().unwrap_or(false),
                        id,
                    }
                })
                .collect(),
            next,
        })
    }

    async fn run_one(&self, id: &AgentCommandId) -> Result<Duration, CommandLedgerError> {
        self.calls.lock().unwrap().push(id.clone());
        let Some(behavior) = self.jobs.lock().unwrap().get(id).cloned() else {
            return Ok(Duration::ZERO);
        };
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let _active = ActiveCall(self);
        match behavior {
            Behavior::Complete => {}
            Behavior::Blocked => return Ok(Duration::ZERO),
            Behavior::Deferred(delay) => return Ok(delay),
            Behavior::Wait(release) => release.notified().await,
            Behavior::Fail => return Err(CommandLedgerError::Unavailable),
            Behavior::Conflict => return Err(CommandLedgerError::Conflict),
            Behavior::Stale => return Err(CommandLedgerError::StaleClaim),
            Behavior::Panic => panic!("simulated worker panic"),
        }
        self.jobs.lock().unwrap().remove(id);
        Ok(Duration::ZERO)
    }
}

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn new_command_starts_within_quarter_second_without_busy_polling() {
    let probe = Arc::new(Probe::default());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    let visitor = probe.put("cmd_fresh", Behavior::Complete);
    tokio::time::advance(Duration::from_millis(249)).await;
    settle().await;
    assert_eq!(probe.count(&visitor), 0);
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(probe.count(&visitor), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn slow_command_does_not_block_new_visitors_or_run_twice() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    let slow = probe.put("cmd_a", Behavior::Wait(release.clone()));
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.count(&slow), 1);
    let visitor = probe.put("cmd_b", Behavior::Complete);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(probe.count(&visitor), 1);
    assert!(!probe.jobs.lock().unwrap().contains_key(&visitor));
    assert_eq!(probe.count(&slow), 1);
    shutdown.send(true).unwrap();
    release.notify_one();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn freed_slots_start_queued_commands_without_waiting_for_other_work() {
    let probe = Arc::new(Probe::default());
    let release_a = Arc::new(Notify::new());
    let release_b = Arc::new(Notify::new());
    probe.put("cmd_a", Behavior::Wait(release_a.clone()));
    probe.put("cmd_b", Behavior::Wait(release_b.clone()));
    let queued = probe.put("cmd_c", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.count(&queued), 0);
    release_b.notify_one();
    settle().await;
    assert_eq!(probe.count(&queued), 1);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    shutdown.send(true).unwrap();
    release_a.notify_one();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn discovery_errors_and_failed_or_panicked_commands_do_not_stop_scheduling() {
    let probe = Arc::new(Probe::default());
    probe.fail_poll.store(true, Ordering::SeqCst);
    probe.put("cmd_a", Behavior::Fail);
    probe.put("cmd_b", Behavior::Panic);
    let visitor = probe.put("cmd_c", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.count(&visitor), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(probe.count(&visitor), 1);
    assert!(!scheduler.is_finished());
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_drains_for_a_bounded_time_without_starting_queued_work() {
    let probe = Arc::new(Probe::default());
    probe.put("cmd_a", Behavior::Wait(Arc::new(Notify::new())));
    let queued = probe.put("cmd_b", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(1).spawn(signal);
    settle().await;
    shutdown.send(true).unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    scheduler.await.unwrap();
    assert_eq!(probe.count(&queued), 0);
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn blocked_first_page_cannot_starve_later_commands() {
    let probe = Arc::new(Probe::default());
    for index in 0..usize::from(PAGE_SIZE) {
        probe.put(&format!("cmd_{index:03}"), Behavior::Blocked);
    }
    let later = probe.put("cmd_zzz", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.count(&later), 0);
    probe.fail_poll.store(true, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(probe.count(&later), 0);
    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
    }
    assert_eq!(probe.count(&later), 1);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn deferred_messages_release_slots_for_a_later_page_interrupt() {
    let probe = Arc::new(Probe::default());
    let mut waiting = Vec::new();
    for index in 0..60 {
        waiting.push(probe.put(
            &format!("cmd_{index:03}"),
            Behavior::Deferred(Duration::from_secs(15)),
        ));
    }
    let stop = probe.put("cmd_stop", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(8).spawn(signal);
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    assert_eq!(probe.count(&stop), 0);
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    tokio::time::advance(POLL_INTERVAL).await;
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        probe.count(&stop),
        1,
        "Stop must not wait behind fifteen-second retries"
    );
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    // Discovery may traverse cooling rows, but does not execute them early.
    for _ in 0..4 {
        tokio::time::advance(POLL_INTERVAL).await;
        settle().await;
    }
    assert!(waiting.iter().all(|id| probe.count(id) == 1));
    assert!(probe.peak.load(Ordering::SeqCst) <= 8);
    tokio::time::advance(Duration::from_secs(15)).await;
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    assert!(waiting.iter().any(|id| probe.count(id) > 1));
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[path = "scheduler_pacing_tests.rs"]
mod pacing;
