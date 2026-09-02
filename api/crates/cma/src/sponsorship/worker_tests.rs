use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

use super::*;

fn settings(page_size: u32, max_pages: usize, concurrency: usize) -> SponsorshipWorkerSettings {
    SponsorshipWorkerSettings::new(Duration::from_secs(1), page_size, max_pages, concurrency)
        .unwrap()
}

#[test]
fn worker_bounds_and_internal_authority_are_exact() {
    for invalid in [
        SponsorshipWorkerSettings::new(Duration::ZERO, 1, 1, 1),
        SponsorshipWorkerSettings::new(Duration::from_secs(301), 1, 1, 1),
        SponsorshipWorkerSettings::new(Duration::from_secs(1), 0, 1, 1),
        SponsorshipWorkerSettings::new(Duration::from_secs(1), 201, 1, 1),
        SponsorshipWorkerSettings::new(Duration::from_secs(1), 1, 0, 1),
        SponsorshipWorkerSettings::new(Duration::from_secs(1), 1, 101, 1),
        SponsorshipWorkerSettings::new(Duration::from_secs(1), 1, 1, 0),
        SponsorshipWorkerSettings::new(Duration::from_secs(1), 1, 1, 33),
    ] {
        assert!(matches!(
            invalid,
            Err(SponsorshipWorkerError::InvalidConfiguration(_))
        ));
    }
    assert_eq!(settings(200, 100, 32).page_size, 200);
    assert!(validate_retiring_key_ids(vec!["old-key".into()]).is_ok());
    assert!(validate_retiring_key_ids(vec!["old-key".into(), "old-key".into()]).is_err());
    assert!(validate_retiring_key_ids(
        (0..=MAX_RETIRING_ENCRYPTION_KEYS)
            .map(|index| format!("old-{index}"))
            .collect()
    )
    .is_err());

    let caller = recovery_caller();
    assert!(caller
        .authorization
        .allows(Capability::ReconcileSponsoredUsage));
    assert!(!caller.authorization.allows(Capability::ConfigurePlatform));
    assert_eq!(caller.authorization.capabilities().count(), 1);
}

#[derive(Default)]
struct PagingBackend {
    fences: AtomicUsize,
    requests: Mutex<Vec<(Option<String>, Option<String>)>>,
}

#[async_trait]
impl RecoveryBackend for PagingBackend {
    fn time_fence(&self) -> Result<String, SponsorshipWorkerError> {
        let sequence = self.fences.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(format!("2026-08-29T00:00:0{sequence}Z"))
    }

    async fn candidates(
        &self,
        lane: &RecoveryLane,
        as_of: Option<&str>,
        page: SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<RecoveryCandidate>, SponsorshipWorkerError> {
        assert_eq!(lane, &RecoveryLane::Grants);
        self.requests
            .lock()
            .unwrap()
            .push((as_of.map(str::to_string), page.cursor().map(str::to_string)));
        Ok(SponsorshipRecoveryPage {
            items: vec![RecoveryCandidate::Grant(SponsoredGrantId::new())],
            next: page.cursor().is_none().then(|| "cursor-1".to_string()),
        })
    }

    async fn run_candidate(
        &self,
        _candidate: RecoveryCandidate,
        _as_of: Option<&str>,
    ) -> Result<CandidateOutcome, SponsorshipWorkerError> {
        Ok(CandidateOutcome::Completed)
    }
}

#[tokio::test]
async fn grant_cursor_retains_one_time_fence_until_traversal_finishes() {
    let backend = Arc::new(PagingBackend::default());
    let scheduler =
        RecoveryLaneScheduler::new(backend.clone(), RecoveryLane::Grants, settings(1, 1, 1));

    let first = scheduler.run_pass().await;
    assert!(first.page_budget_exhausted);
    let second = scheduler.run_pass().await;
    assert!(!second.page_budget_exhausted);
    let third = scheduler.run_pass().await;
    assert!(third.page_budget_exhausted);

    let requests = backend.requests.lock().unwrap();
    assert_eq!(
        requests.as_slice(),
        [
            (Some("2026-08-29T00:00:01Z".into()), None),
            (Some("2026-08-29T00:00:01Z".into()), Some("cursor-1".into())),
            (Some("2026-08-29T00:00:02Z".into()), None),
        ]
    );
    assert_eq!(backend.fences.load(Ordering::SeqCst), 2);
}

struct ConcurrencyBackend {
    active: AtomicUsize,
    maximum: AtomicUsize,
    calls: AtomicUsize,
}

impl Default for ConcurrencyBackend {
    fn default() -> Self {
        Self {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl RecoveryBackend for ConcurrencyBackend {
    fn time_fence(&self) -> Result<String, SponsorshipWorkerError> {
        unreachable!()
    }

    async fn candidates(
        &self,
        _lane: &RecoveryLane,
        _as_of: Option<&str>,
        _page: SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<RecoveryCandidate>, SponsorshipWorkerError> {
        Ok(SponsorshipRecoveryPage {
            items: (0..9)
                .map(|_| RecoveryCandidate::Credential(SponsoredCredentialId::new()))
                .collect(),
            next: None,
        })
    }

    async fn run_candidate(
        &self,
        _candidate: RecoveryCandidate,
        _as_of: Option<&str>,
    ) -> Result<CandidateOutcome, SponsorshipWorkerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CandidateOutcome::Completed)
    }
}

#[tokio::test]
async fn every_page_respects_the_configured_concurrency_bound() {
    let backend = Arc::new(ConcurrencyBackend::default());
    let scheduler = RecoveryLaneScheduler::new(
        backend.clone(),
        RecoveryLane::Credentials,
        settings(20, 1, 3),
    );
    let stats = scheduler.run_pass().await;
    assert_eq!(stats.candidates, 9);
    assert_eq!(stats.completed, 9);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 9);
    assert_eq!(backend.maximum.load(Ordering::SeqCst), 3);
}

struct TickBackend {
    calls: AtomicUsize,
    first_started: Notify,
    release_first: Notify,
}

impl Default for TickBackend {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
        }
    }
}

#[async_trait]
impl RecoveryBackend for TickBackend {
    fn time_fence(&self) -> Result<String, SponsorshipWorkerError> {
        unreachable!()
    }

    async fn candidates(
        &self,
        _lane: &RecoveryLane,
        _as_of: Option<&str>,
        _page: SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<RecoveryCandidate>, SponsorshipWorkerError> {
        Ok(SponsorshipRecoveryPage {
            items: vec![RecoveryCandidate::Credential(SponsoredCredentialId::new())],
            next: None,
        })
    }

    async fn run_candidate(
        &self,
        _candidate: RecoveryCandidate,
        _as_of: Option<&str>,
    ) -> Result<CandidateOutcome, SponsorshipWorkerError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.first_started.notify_one();
            self.release_first.notified().await;
        }
        Ok(CandidateOutcome::Completed)
    }
}

#[tokio::test(start_paused = true)]
async fn a_slow_pass_skips_missed_ticks_instead_of_bursting() {
    let backend = Arc::new(TickBackend::default());
    let scheduler = Arc::new(RecoveryLaneScheduler::new(
        backend.clone(),
        RecoveryLane::Credentials,
        settings(1, 1, 1),
    ));
    let (_shutdown, receiver) = tokio::sync::watch::channel(false);
    let handle = scheduler.spawn(receiver);
    backend.first_started.notified().await;

    tokio::time::advance(Duration::from_secs(10)).await;
    backend.release_first.notify_one();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        2,
        "ten missed intervals produce one catch-up pass, not a burst"
    );
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);

    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    handle.abort();
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_new_passes_but_allows_the_active_candidate_to_settle() {
    let backend = Arc::new(TickBackend::default());
    let scheduler = Arc::new(RecoveryLaneScheduler::new(
        backend.clone(),
        RecoveryLane::Credentials,
        settings(1, 1, 1),
    ));
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let handle = scheduler.spawn(receiver);
    backend.first_started.notified().await;

    shutdown.send(true).unwrap();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert!(!handle.is_finished(), "the active candidate is drained");
    backend.release_first.notify_one();
    handle.await.unwrap();

    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "shutdown never schedules another pass"
    );
}

#[tokio::test(start_paused = true)]
async fn owned_shutdown_aborts_a_pass_before_the_process_deadline() {
    let backend = Arc::new(TickBackend::default());
    let scheduler = Arc::new(RecoveryLaneScheduler::new(
        backend.clone(),
        RecoveryLane::Credentials,
        settings(1, 1, 1),
    ));
    let (shutdown, _receiver) = tokio::sync::watch::channel(false);
    let workers = SponsorshipWorkers {
        handles: vec![scheduler.spawn(shutdown.subscribe())],
        shutdown,
    };
    backend.first_started.notified().await;

    let shutdown_task = tokio::spawn(workers.shutdown());
    tokio::task::yield_now().await;
    tokio::time::advance(WORKER_DRAIN_TIMEOUT + Duration::from_secs(1)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }

    assert!(
        shutdown_task.is_finished(),
        "the owned worker is aborted inside its drain budget"
    );
    shutdown_task.await.unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

struct CandidateFailureBackend {
    calls: AtomicUsize,
}

#[async_trait]
impl RecoveryBackend for CandidateFailureBackend {
    fn time_fence(&self) -> Result<String, SponsorshipWorkerError> {
        unreachable!()
    }

    async fn candidates(
        &self,
        _lane: &RecoveryLane,
        _as_of: Option<&str>,
        _page: SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<RecoveryCandidate>, SponsorshipWorkerError> {
        Ok(SponsorshipRecoveryPage {
            items: (0..3)
                .map(|_| RecoveryCandidate::Activation(LaunchId::new()))
                .collect(),
            next: None,
        })
    }

    async fn run_candidate(
        &self,
        _candidate: RecoveryCandidate,
        _as_of: Option<&str>,
    ) -> Result<CandidateOutcome, SponsorshipWorkerError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Err(SponsorshipWorkerError::InvalidConfiguration(
                "injected candidate failure".into(),
            ))
        } else {
            Ok(CandidateOutcome::Completed)
        }
    }
}

#[tokio::test]
async fn one_candidate_failure_does_not_block_its_siblings() {
    let backend = Arc::new(CandidateFailureBackend {
        calls: AtomicUsize::new(0),
    });
    let scheduler = RecoveryLaneScheduler::new(
        backend.clone(),
        RecoveryLane::Activations,
        settings(3, 1, 3),
    );
    let stats = scheduler.run_pass().await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    assert_eq!(stats.completed, 2);
    assert_eq!(stats.errors, 1);
}
