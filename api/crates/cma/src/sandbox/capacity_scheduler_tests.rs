use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex as StdMutex,
};

use layer2_core::AgentId;
use tokio::sync::Semaphore;

use super::*;

struct Probe {
    requests: Vec<RecoveryRequest>,
    streams: StdMutex<Vec<RecoveryStream>>,
    attempts: StdMutex<Vec<RecoveryRequest>>,
    fail_discovery: AtomicBool,
    installed: AtomicBool,
    installation_checks: AtomicUsize,
    block: AtomicBool,
    release: Semaphore,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl Probe {
    fn new(count: usize) -> Arc<Self> {
        Arc::new(Self {
            requests: (0..count)
                .map(|i| RecoveryRequest {
                    agent_id: AgentId(format!("agt_recovery_scheduler_{i}")),
                    creation_key: format!("original-{i}"),
                    creation_fingerprint: "a".repeat(64),
                    failure_seq: 3,
                    requested_at: "2026-11-11T00:00:00Z".into(),
                })
                .collect(),
            streams: StdMutex::new(Vec::new()),
            attempts: StdMutex::new(Vec::new()),
            fail_discovery: AtomicBool::new(false),
            installed: AtomicBool::new(true),
            installation_checks: AtomicUsize::new(0),
            block: AtomicBool::new(false),
            release: Semaphore::new(0),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }

    fn sweep(self: &Arc<Self>) -> Arc<CapacityRecoverySweep> {
        Arc::new(CapacityRecoverySweep {
            work: self.clone(),
            state: Mutex::new(DiscoveryState::default()),
        })
    }
}

struct Active<'a>(&'a Probe);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl AcceptedRecovery for Probe {
    async fn installed(&self) -> Result<bool, RecoveryError> {
        self.installation_checks.fetch_add(1, Ordering::SeqCst);
        Ok(self.installed.load(Ordering::SeqCst))
    }

    async fn discover(
        &self,
        lane: RecoveryStream,
        _: Option<&RecoveryCursor>,
        _: &str,
    ) -> Result<RecoveryPage, RecoveryError> {
        self.streams.lock().unwrap().push(lane);
        if self.fail_discovery.swap(false, Ordering::SeqCst) {
            return Err(RecoveryError::Unavailable);
        }
        Ok(RecoveryPage {
            requests: self.requests.clone(),
            next: None,
        })
    }

    async fn run(&self, request: &RecoveryRequest) -> Result<bool, RecoveryError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let _active = Active(self);
        self.attempts.lock().unwrap().push(request.clone());
        if self.block.load(Ordering::SeqCst) {
            self.release.acquire().await.unwrap().forget();
        }
        if request.agent_id.as_str().ends_with("_0") {
            Err(RecoveryError::Conflict)
        } else {
            Ok(true)
        }
    }
}

#[tokio::test]
async fn refusal_is_isolated_and_one_page_is_retained_until_consumed() {
    let probe = Probe::new(32);
    let sweep = probe.sweep();
    let now = SystemTime::now();
    let mut completed = 0;
    for _ in 0..16 {
        completed += sweep.run_due(now).await.unwrap();
    }
    assert_eq!(completed, 31);
    assert_eq!(*probe.attempts.lock().unwrap(), probe.requests);
    assert_eq!(*probe.streams.lock().unwrap(), [RecoveryStream::Admitted]);
    sweep.run_due(now).await.unwrap();
    assert_eq!(
        *probe.streams.lock().unwrap(),
        [RecoveryStream::Admitted, RecoveryStream::ExpiredLease]
    );
}

#[tokio::test]
async fn overlapping_sweeps_never_exceed_two_claims_or_drop_the_queue() {
    let probe = Probe::new(4);
    probe.block.store(true, Ordering::SeqCst);
    let sweep = probe.sweep();
    let running = {
        let sweep = sweep.clone();
        tokio::spawn(async move { sweep.run_due(SystemTime::now()).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while probe.active.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sweep.run_due(SystemTime::now()).await.unwrap(), 0);
    assert_eq!(probe.attempts.lock().unwrap().len(), 2);
    probe.block.store(false, Ordering::SeqCst);
    probe.release.add_permits(2);
    assert_eq!(running.await.unwrap().unwrap(), 1);
    assert_eq!(sweep.run_due(SystemTime::now()).await.unwrap(), 2);
    assert_eq!(probe.attempts.lock().unwrap().len(), 4);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_discovery_refuses_before_executing_any_candidate() {
    let probe = Probe::new(33);
    assert!(matches!(
        probe.sweep().run_due(SystemTime::now()).await,
        Err(RecoveryError::InvalidEvidence)
    ));
    assert!(probe.attempts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_discovery_does_not_starve_the_other_lane() {
    let probe = Probe::new(2);
    probe.fail_discovery.store(true, Ordering::SeqCst);
    let sweep = probe.sweep();
    assert!(matches!(
        sweep.run_due(SystemTime::now()).await,
        Err(RecoveryError::Unavailable)
    ));
    assert_eq!(sweep.run_due(SystemTime::now()).await.unwrap(), 1);
    assert_eq!(
        *probe.streams.lock().unwrap(),
        [RecoveryStream::Admitted, RecoveryStream::ExpiredLease]
    );
}

#[tokio::test]
async fn restart_rediscovers_exact_original_requests_without_new_admission() {
    let probe = Probe::new(2);
    for _ in 0..2 {
        assert_eq!(probe.sweep().run_due(SystemTime::now()).await.unwrap(), 1);
    }
    let attempts = probe.attempts.lock().unwrap();
    assert_eq!(&attempts[..2], &attempts[2..]);
    for (original, retry) in attempts[..2].iter().zip(&attempts[2..]) {
        assert_eq!(original.key().unwrap(), retry.key().unwrap());
    }
}

#[tokio::test]
async fn cancellation_drops_only_local_hints_and_leaves_no_running_claim_task() {
    let probe = Probe::new(3);
    probe.block.store(true, Ordering::SeqCst);
    let sweep = probe.sweep();
    let task = {
        let sweep = sweep.clone();
        tokio::spawn(async move { sweep.run_due(SystemTime::now()).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while probe.active.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    probe.block.store(false, Ordering::SeqCst);
    assert_eq!(probe.sweep().run_due(SystemTime::now()).await.unwrap(), 1);
    let attempts = probe.attempts.lock().unwrap();
    assert_eq!(&attempts[..2], &attempts[2..]);
}

#[tokio::test]
async fn schema_absence_defers_reads_but_later_installation_is_observed() {
    let probe = Probe::new(2);
    probe.installed.store(false, Ordering::SeqCst);
    let sweep = probe.sweep();
    assert_eq!(sweep.run_due(SystemTime::now()).await.unwrap(), 0);
    assert!(probe.streams.lock().unwrap().is_empty());
    assert!(probe.attempts.lock().unwrap().is_empty());
    probe.installed.store(true, Ordering::SeqCst);
    assert_eq!(sweep.run_due(SystemTime::now()).await.unwrap(), 1);
    assert_eq!(sweep.run_due(SystemTime::now()).await.unwrap(), 1);
    assert_eq!(probe.installation_checks.load(Ordering::SeqCst), 2);
}
