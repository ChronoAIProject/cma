use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use tokio::sync::Notify;

use super::*;

#[derive(Clone)]
enum Behavior {
    Complete,
    Wait(Arc<Notify>),
    Fail,
    Panic,
    Unclaimed,
}

#[derive(Default)]
struct Probe {
    jobs: Mutex<BTreeMap<AgentId, Behavior>>,
    calls: Mutex<Vec<(AgentId, SystemTime)>>,
    completed: Mutex<BTreeSet<AgentId>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    dropped: AtomicUsize,
    polls: AtomicUsize,
    reconciles: AtomicUsize,
    fail_poll: AtomicBool,
    block_poll: AtomicBool,
    block_reconcile: AtomicBool,
    poll_release: Mutex<Option<Arc<Notify>>>,
}

impl Probe {
    fn put(&self, name: &str, behavior: Behavior) -> AgentId {
        let id = AgentId(name.into());
        self.jobs.lock().unwrap().insert(id.clone(), behavior);
        id
    }

    fn calls(&self, id: &AgentId) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(called, _)| called == id)
            .count()
    }

    fn scheduler(self: &Arc<Self>, concurrency: usize) -> AgentScheduler {
        AgentScheduler {
            worker: self.clone(),
            sweep_interval: Duration::from_secs(1),
            concurrency,
            drain_timeout: Duration::from_secs(5),
            admitted: Arc::new(Notify::new()),
        }
    }
}

struct ActiveCall<'a>(&'a Probe);

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl AgentScheduleWork for Probe {
    async fn due(&self, _now: SystemTime, limit: u16) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if self.block_poll.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.fail_poll.swap(false, Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable("probe poll failed".into()));
        }
        // A stale discovery result may still offer locally running IDs.
        let ids = self
            .jobs
            .lock()
            .unwrap()
            .keys()
            .take(usize::from(limit))
            .cloned()
            .collect();
        let release = self.poll_release.lock().unwrap().take();
        if let Some(release) = release {
            release.notified().await;
        }
        Ok(ids)
    }

    async fn reconcile(&self, _now: SystemTime) -> Result<usize, SandboxStoreError> {
        self.reconciles.fetch_add(1, Ordering::SeqCst);
        if self.block_reconcile.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(0)
    }

    async fn run_one(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError> {
        self.calls.lock().unwrap().push((id.clone(), now));
        let Some(behavior) = self.jobs.lock().unwrap().get(id).cloned() else {
            return Ok(false);
        };
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let _active = ActiveCall(self);
        match behavior {
            Behavior::Wait(release) => release.notified().await,
            Behavior::Fail => {
                return Err(SandboxStoreError::Unavailable("probe work failed".into()))
            }
            Behavior::Panic => panic!("probe work panicked"),
            Behavior::Unclaimed => return Ok(false),
            Behavior::Complete => {}
        }
        self.jobs.lock().unwrap().remove(id);
        self.completed.lock().unwrap().insert(id.clone());
        Ok(true)
    }
}

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn slow_authentication_does_not_block_new_work_or_connection_reconciliation() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    let slow = probe.put("agt_a", Behavior::Wait(release.clone()));
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.calls(&slow), 1);

    let next = probe.put("agt_b", Behavior::Complete);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(probe.completed.lock().unwrap().contains(&next));
    assert_eq!(probe.calls(&slow), 1);
    assert_eq!(probe.active.load(Ordering::SeqCst), 1);
    assert!(probe.reconciles.load(Ordering::SeqCst) >= 2);
    shutdown.send(true).unwrap();
    release.notify_one();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn completed_slots_refill_without_a_batch_barrier_or_a_new_tick() {
    let probe = Arc::new(Probe::default());
    let release_a = Arc::new(Notify::new());
    let release_b = Arc::new(Notify::new());
    let a = probe.put("agt_a", Behavior::Wait(release_a.clone()));
    let b = probe.put("agt_b", Behavior::Wait(release_b.clone()));
    let c = probe.put("agt_c", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 2);
    assert_eq!(probe.calls(&c), 0);
    let released_at = SystemTime::now();
    release_b.notify_one();
    settle().await;

    assert!(probe.completed.lock().unwrap().contains(&b));
    assert!(probe.completed.lock().unwrap().contains(&c));
    assert_eq!(probe.calls(&a), 1);
    assert_eq!(probe.polls.load(Ordering::SeqCst), 1);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    let c_time = probe
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|(id, _)| id == &c)
        .unwrap()
        .1;
    assert!(
        c_time >= released_at,
        "queued work must use a fresh claim time"
    );
    shutdown.send(true).unwrap();
    release_a.notify_one();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn failed_poll_and_panicked_or_failed_work_do_not_occupy_slots_forever() {
    let probe = Arc::new(Probe::default());
    probe.fail_poll.store(true, Ordering::SeqCst);
    let panic = probe.put("agt_a", Behavior::Panic);
    let failure = probe.put("agt_b", Behavior::Fail);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.calls(&panic), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(probe.calls(&panic), 1);
    assert_eq!(probe.calls(&failure), 1);
    probe.put("agt_a", Behavior::Complete);
    probe.put("agt_b", Behavior::Complete);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(probe.completed.lock().unwrap().contains(&panic));
    assert!(probe.completed.lock().unwrap().contains(&failure));
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unavailable_durable_claim_does_not_complete_work_or_busy_loop() {
    let probe = Arc::new(Probe::default());
    let id = probe.put("agt_a", Behavior::Unclaimed);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert_eq!(probe.calls(&id), 1);
    assert!(probe.completed.lock().unwrap().is_empty());
    assert!(probe.jobs.lock().unwrap().contains_key(&id));
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(probe.calls(&id), 2);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn blocked_reconciliation_does_not_block_agent_work_and_shutdown_is_bounded() {
    let probe = Arc::new(Probe::default());
    probe.block_reconcile.store(true, Ordering::SeqCst);
    let id = probe.put("agt_a", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    assert!(probe.completed.lock().unwrap().contains(&id));
    tokio::time::advance(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(probe.reconciles.load(Ordering::SeqCst), 1);
    shutdown.send(true).unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn blocked_discovery_does_not_block_reconciliation_or_shutdown() {
    let probe = Arc::new(Probe::default());
    probe.block_poll.store(true, Ordering::SeqCst);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(probe.polls.load(Ordering::SeqCst), 1);
    assert!(probe.reconciles.load(Ordering::SeqCst) >= 2);
    shutdown.send(true).unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_drains_in_flight_work_without_starting_queued_intents() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    let current = probe.put("agt_a", Behavior::Wait(release.clone()));
    let queued = probe.put("agt_b", Behavior::Complete);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(1).spawn(signal);
    settle().await;
    shutdown.send(true).unwrap();
    settle().await;
    assert!(!scheduler.is_finished());
    release.notify_one();
    scheduler.await.unwrap();
    assert!(probe.completed.lock().unwrap().contains(&current));
    assert_eq!(probe.calls(&queued), 0);
    assert!(probe.jobs.lock().unwrap().contains_key(&queued));
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn drain_deadline_aborts_and_joins_owned_work_preserving_intents() {
    let probe = Arc::new(Probe::default());
    let id = probe.put("agt_a", Behavior::Wait(Arc::new(Notify::new())));
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(1).spawn(signal);
    settle().await;
    shutdown.send(true).unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    scheduler.await.unwrap();
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
    assert!(probe.completed.lock().unwrap().is_empty());
    assert!(probe.jobs.lock().unwrap().contains_key(&id));
}

#[tokio::test(start_paused = true)]
async fn aborting_outer_scheduler_cancels_owned_agent_tasks() {
    let probe = Arc::new(Probe::default());
    let id = probe.put("agt_a", Behavior::Wait(Arc::new(Notify::new())));
    let (_shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(1).spawn(signal);
    settle().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 1);
    scheduler.abort();
    assert!(scheduler.await.unwrap_err().is_cancelled());
    settle().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
    assert!(probe.jobs.lock().unwrap().contains_key(&id));
}

#[tokio::test(start_paused = true)]
async fn already_signalled_shutdown_starts_no_work() {
    let probe = Arc::new(Probe::default());
    probe.put("agt_a", Behavior::Complete);
    let (_shutdown, signal) = watch::channel(true);
    probe.scheduler(1).spawn(signal).await.unwrap();
    assert_eq!(probe.polls.load(Ordering::SeqCst), 0);
    assert_eq!(probe.reconciles.load(Ordering::SeqCst), 0);
    assert!(probe.calls.lock().unwrap().is_empty());
}

#[path = "agent_scheduler_wake_tests.rs"]
mod wake_tests;
