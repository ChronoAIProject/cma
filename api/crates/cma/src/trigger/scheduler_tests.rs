use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    seconds_from_rfc3339, ActivityQuery, ActivityRecord, ActivityStore, CleanupTarget, Launch,
    LaunchId, LaunchState, StoreError, TriggerPage, TriggerPageRequest, TriggerServiceError,
    TriggerStoreError, WorkspaceManifest, WorkspaceRecord,
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use super::super::worker::tests::{launch_fixture, service_for_launch};
use super::*;

fn candidate() -> Launch {
    let mut launch = launch_fixture();
    launch.id = LaunchId::new();
    launch
}

#[derive(Debug, Clone, Copy)]
enum PagingMode {
    Advancing,
    Looping,
}

struct PagingBackend {
    mode: PagingMode,
    calls: AtomicUsize,
    cursors: Mutex<Vec<Option<String>>>,
}

impl PagingBackend {
    fn new(mode: PagingMode) -> Self {
        Self {
            mode,
            calls: AtomicUsize::new(0),
            cursors: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SchedulerBackend for PagingBackend {
    async fn candidates(
        &self,
        _lane: SchedulerLane,
        _pass_boundary: &str,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.cursors.lock().unwrap().push(page.cursor);
        let next = match self.mode {
            PagingMode::Advancing => format!("cursor-{call}"),
            PagingMode::Looping => "same-cursor".to_string(),
        };
        Ok(TriggerPage {
            items: vec![candidate()],
            next: Some(next),
        })
    }

    async fn run_candidate(
        &self,
        _lane: SchedulerLane,
        _launch: Launch,
        _correlation_id: String,
    ) -> Result<CandidateOutcome, TriggerServiceError> {
        Ok(CandidateOutcome::Completed)
    }
}

fn scheduler(
    backend: Arc<dyn SchedulerBackend>,
    max_pages: usize,
    concurrency: usize,
) -> TriggerScheduler {
    TriggerScheduler {
        backend,
        settings: SchedulerSettings {
            interval: Duration::from_secs(1),
            page_size: 2,
            max_pages,
            concurrency,
        },
    }
}

#[tokio::test]
async fn pagination_stops_at_the_configured_page_budget() {
    let backend = Arc::new(PagingBackend::new(PagingMode::Advancing));
    let scheduler = scheduler(backend.clone(), 3, 2);

    let stats = scheduler.run_pass(SchedulerLane::Effects).await;

    assert_eq!(stats.pages, 3);
    assert_eq!(stats.candidates, 3);
    assert_eq!(stats.completed, 3);
    assert_eq!(stats.errors, 0);
    assert!(stats.page_budget_exhausted);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        backend.cursors.lock().unwrap().as_slice(),
        [None, Some("cursor-1".into()), Some("cursor-2".into())]
    );
}

#[tokio::test]
async fn a_repeated_cursor_is_detected_and_cannot_loop_forever() {
    let backend = Arc::new(PagingBackend::new(PagingMode::Looping));
    let scheduler = scheduler(backend.clone(), 50, 2);

    let stats = scheduler.run_pass(SchedulerLane::Cleanup).await;

    assert_eq!(stats.pages, 2);
    assert_eq!(stats.errors, 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

struct BlockingBackend {
    candidates: usize,
    active: AtomicUsize,
    maximum: AtomicUsize,
    release: AtomicBool,
}

impl BlockingBackend {
    fn observe_maximum(&self, active: usize) {
        let mut maximum = self.maximum.load(Ordering::SeqCst);
        while active > maximum {
            match self
                .maximum
                .compare_exchange(maximum, active, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(observed) => maximum = observed,
            }
        }
    }
}

#[async_trait]
impl SchedulerBackend for BlockingBackend {
    async fn candidates(
        &self,
        _lane: SchedulerLane,
        _pass_boundary: &str,
        _page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(TriggerPage {
            items: (0..self.candidates).map(|_| candidate()).collect(),
            next: None,
        })
    }

    async fn run_candidate(
        &self,
        _lane: SchedulerLane,
        _launch: Launch,
        _correlation_id: String,
    ) -> Result<CandidateOutcome, TriggerServiceError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.observe_maximum(active);
        while !self.release.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CandidateOutcome::Completed)
    }
}

#[tokio::test]
async fn candidate_concurrency_never_exceeds_the_configured_bound() {
    let backend = Arc::new(BlockingBackend {
        candidates: 7,
        active: AtomicUsize::new(0),
        maximum: AtomicUsize::new(0),
        release: AtomicBool::new(false),
    });
    let scheduler = Arc::new(scheduler(backend.clone(), 1, 2));
    let pass = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move { scheduler.run_pass(SchedulerLane::Effects).await })
    };

    tokio::time::timeout(Duration::from_secs(2), async {
        while backend.active.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the configured parallel slots became active");
    assert_eq!(backend.maximum.load(Ordering::SeqCst), 2);
    backend.release.store(true, Ordering::SeqCst);

    let stats = pass.await.unwrap();
    assert_eq!(stats.completed, 7);
    assert_eq!(backend.maximum.load(Ordering::SeqCst), 2);
}

struct IndependentLaneBackend {
    effect_started: AtomicBool,
    effect_release: Semaphore,
    capability_runs: AtomicUsize,
}

#[async_trait]
impl SchedulerBackend for IndependentLaneBackend {
    async fn candidates(
        &self,
        _lane: SchedulerLane,
        _pass_boundary: &str,
        _page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        Ok(TriggerPage {
            items: vec![candidate()],
            next: None,
        })
    }

    async fn run_candidate(
        &self,
        lane: SchedulerLane,
        _launch: Launch,
        _correlation_id: String,
    ) -> Result<CandidateOutcome, TriggerServiceError> {
        match lane {
            SchedulerLane::Effects => {
                self.effect_started.store(true, Ordering::SeqCst);
                self.effect_release.acquire().await.unwrap().forget();
            }
            SchedulerLane::Capabilities => {
                self.capability_runs.fetch_add(1, Ordering::SeqCst);
            }
            SchedulerLane::Cleanup => {}
        }
        Ok(CandidateOutcome::Completed)
    }
}

#[tokio::test]
async fn capability_pass_is_not_delayed_by_a_blocked_provider_effect() {
    let backend = Arc::new(IndependentLaneBackend {
        effect_started: AtomicBool::new(false),
        effect_release: Semaphore::new(0),
        capability_runs: AtomicUsize::new(0),
    });
    let scheduler = Arc::new(scheduler(backend.clone(), 1, 1));
    let effects = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move { scheduler.run_pass(SchedulerLane::Effects).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while !backend.effect_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the effect lane reached its provider boundary");

    let capability = tokio::time::timeout(
        Duration::from_secs(2),
        scheduler.run_pass(SchedulerLane::Capabilities),
    )
    .await
    .expect("capability maintenance remained independently schedulable");
    assert_eq!(capability.completed, 1);
    assert_eq!(backend.capability_runs.load(Ordering::SeqCst), 1);

    backend.effect_release.add_permits(1);
    assert_eq!(effects.await.unwrap().completed, 1);
}

struct RecoveringBackend {
    page_calls: AtomicUsize,
}

#[async_trait]
impl SchedulerBackend for RecoveringBackend {
    async fn candidates(
        &self,
        _lane: SchedulerLane,
        _pass_boundary: &str,
        _page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        if self.page_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(TriggerStoreError::Unavailable("simulated page outage".into()).into());
        }
        Ok(TriggerPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn run_candidate(
        &self,
        _lane: SchedulerLane,
        _launch: Launch,
        _correlation_id: String,
    ) -> Result<CandidateOutcome, TriggerServiceError> {
        Ok(CandidateOutcome::NoWork)
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_pass_does_not_terminate_the_recurring_lane() {
    let backend = Arc::new(RecoveringBackend {
        page_calls: AtomicUsize::new(0),
    });
    let scheduler = Arc::new(scheduler(backend.clone(), 1, 1));
    let lane = scheduler.spawn_lane(SchedulerLane::Effects);

    while backend.page_calls.load(Ordering::SeqCst) < 1 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    while backend.page_calls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }

    lane.abort();
    assert_eq!(backend.page_calls.load(Ordering::SeqCst), 2);
}

#[derive(Default)]
struct RecordingActivityStore {
    records: AsyncMutex<Vec<ActivityRecord>>,
}

#[async_trait]
impl ActivityStore for RecordingActivityStore {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().await.push(record.clone());
        Ok(())
    }

    async fn recent(&self, _query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        Ok(self.records.lock().await.clone())
    }
}

struct FixedCleanupClock(SystemTime);

impl CleanupClock for FixedCleanupClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

#[tokio::test]
async fn cleanup_dry_run_persists_only_an_exact_plan_and_a_safe_audit_record() {
    let now = "2026-08-27T00:00:00Z";
    let mut launch = launch_fixture();
    launch.state = LaunchState::Cancelled;
    launch.terminal_at = Some("2026-07-01T00:00:00Z".into());
    launch.workspace = Some(WorkspaceRecord {
        baseline: WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap(),
        baseline_object_reference: "retained/baseline.pack".into(),
        baseline_archive_sha256: "a".repeat(64),
        latest_checkpoint: None,
        previous_checkpoint: None,
        workspace_epoch: 1,
        retention_policy_id: "retention-v1".into(),
        delete_after: "2026-08-01T00:00:00Z".into(),
    });
    let service = service_for_launch(launch.clone());
    let activity_store = Arc::new(RecordingActivityStore::default());
    let activity = ActivityService::new(activity_store.clone());
    let clock = Arc::new(FixedCleanupClock(
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds_from_rfc3339(now).unwrap()),
    ));
    let planner = CleanupPlanner::with_clock(
        Arc::clone(&service),
        "cleanup-dry-run-test",
        activity,
        clock,
    )
    .unwrap();

    assert_eq!(
        planner
            .run_once(&launch.id, launch.revision_number, "cleanup-plan-test")
            .await
            .unwrap(),
        CleanupPlanOutcome::Planned
    );
    let stored = service.get_launch(&launch.owner, &launch.id).await.unwrap();
    assert!(stored.lease.is_none());
    let cleanup = stored.cleanup.expect("the dry-run plan is durable");
    assert_eq!(cleanup.completed_at, None);
    assert_eq!(cleanup.plan.targets, vec![CleanupTarget::Baseline]);
    let wire = serde_json::to_string(&cleanup.plan).unwrap();
    for forbidden in ["sandbox", "fork", "branch", "commit", "pull_request"] {
        assert!(
            !wire.contains(forbidden),
            "dry-run named {forbidden}: {wire}"
        );
    }

    let records = activity_store.records.lock().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, ActivityAction::TriggerCleanupPlan);
    assert_eq!(
        records[0].detail.get(DetailKey::CleanupTargetCount),
        Some("1")
    );
    assert_eq!(
        records[0].detail.get(DetailKey::CorrelationId),
        Some("cleanup-plan-test")
    );
    let retained_keys = records[0]
        .detail
        .iter()
        .map(|(key, _)| key)
        .collect::<Vec<_>>();
    assert_eq!(
        retained_keys,
        vec![
            DetailKey::Subject,
            DetailKey::CorrelationId,
            DetailKey::RevisionDigest,
            DetailKey::RepositoryId,
            DetailKey::GithubAccountId,
            DetailKey::ServiceId,
            DetailKey::LaunchState,
            DetailKey::FencingToken,
            DetailKey::CleanupTargetCount,
        ]
    );
}
