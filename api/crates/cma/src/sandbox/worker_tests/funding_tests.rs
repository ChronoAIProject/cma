use super::*;
use crate::sandbox::funding::{FundingError, SandboxFundingAdapter};

struct Funding {
    adapter: Arc<dyn SandboxAdapter>,
    denied: AtomicBool,
    admissions: AtomicUsize,
    settlements: AtomicUsize,
    lose_settlement: AtomicBool,
}
#[async_trait::async_trait]
impl SandboxFundingAdapter for Funding {
    async fn ensure_coverage(
        &self,
        _: &Sandbox,
        _: SystemTime,
    ) -> Result<Option<u64>, FundingError> {
        self.admissions.fetch_add(1, Ordering::SeqCst);
        if self.denied.load(Ordering::SeqCst) {
            Err(FundingError::Unfunded("insufficient credits".into()))
        } else {
            Ok(None)
        }
    }
    async fn settle(&self, _: &Sandbox, _: SystemTime) -> Result<(), FundingError> {
        assert!(
            self.adapter.runtime_inventory().await.unwrap().is_empty(),
            "provider teardown must precede settlement intent"
        );
        self.settlements.fetch_add(1, Ordering::SeqCst);
        if self.lose_settlement.swap(false, Ordering::SeqCst) {
            Err(FundingError::Retryable("funding store unavailable".into()))
        } else {
            Ok(())
        }
    }
    async fn renew_due(&self, _: SystemTime) -> Result<usize, FundingError> {
        Ok(0)
    }
}
fn funded_worker(h: &Harness) -> (Arc<SandboxWorker>, Arc<Funding>) {
    let funding = Arc::new(Funding {
        adapter: h.adapter.clone(),
        denied: AtomicBool::new(true),
        admissions: AtomicUsize::new(0),
        settlements: AtomicUsize::new(0),
        lose_settlement: AtomicBool::new(false),
    });
    (worker_with_funding(h, funding.clone()), funding)
}
fn worker_with_funding(h: &Harness, funding: Arc<dyn SandboxFundingAdapter>) -> Arc<SandboxWorker> {
    let dependencies = SandboxWorkerDependencies {
        store: h.store.clone(),
        adapter: h.adapter.clone(),
        credentials: h.credentials.clone(),
        workspaces: Arc::new(WorkspaceRecorder {
            actions: h.actions.clone(),
        }),
        agents: Arc::new(AgentStopRecorder {
            actions: h.actions.clone(),
        }),
        registry: h.registry.clone(),
        updates: DurableAgentUpdateRecorder::new(h.store.clone()),
        operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
    };
    let mut config =
        SandboxWorkerConfig::new("funding-test", "test", runtime_image(), h.policy.clone());
    config.retry_delay = Duration::from_secs(5);
    SandboxWorker::new_with_funding(dependencies, config, funding)
}

#[tokio::test]
async fn unfunded_provisioning_never_creates_runtime_and_publishes_failure() {
    let h = Harness::new(vec![], 0, Duration::from_secs(3600)).await;
    let sandbox = h.create().await;
    let now = start_time(&sandbox);
    let (worker, funding) = funded_worker(&h);
    worker.run_one(&sandbox.id, now).await.unwrap();
    assert_eq!(funding.admissions.load(Ordering::SeqCst), 1);
    assert!(h.adapter.runtime_inventory().await.unwrap().is_empty());
    assert!(h.stored(&sandbox.id).await.runtime.is_none());
    let events = h.store.sandbox_events(&sandbox.id, 0, 20).await.unwrap();
    assert!(events.iter().any(|e| matches!(&e.kind,SandboxEventKind::WorkStepFailed {failure,..} if failure.detail.contains("prepaid"))));
}
#[tokio::test]
async fn unfunded_retirement_still_tears_down_and_retries_settlement_store_failure() {
    let h = Harness::new(vec![], 0, Duration::from_secs(3600)).await;
    let created = h.create().await;
    let now = start_time(&created);
    let active = h.drive_active(&created.id, now).await;
    let mut retiring = active.clone();
    retiring
        .begin_retirement(rfc3339_at(now + Duration::from_secs(10)))
        .unwrap();
    h.store
        .save_sandbox(active.revision, retiring)
        .await
        .unwrap();
    let (worker, funding) = funded_worker(&h);
    funding.lose_settlement.store(true, Ordering::SeqCst);
    for step in 1..20 {
        worker
            .run_one(&created.id, now + Duration::from_secs(step * 10))
            .await
            .unwrap();
        if h.stored(&created.id).await.status == SandboxStatus::Retired {
            break;
        }
    }
    assert_eq!(h.stored(&created.id).await.status, SandboxStatus::Retired);
    assert_eq!(funding.admissions.load(Ordering::SeqCst), 0);
    assert_eq!(funding.settlements.load(Ordering::SeqCst), 2);
    assert!(h.adapter.runtime_inventory().await.unwrap().is_empty());
}

#[tokio::test]
async fn non_cohort_user_creates_pauses_and_resumes_with_enabled_funding_adapter() {
    let h = Harness::new(vec![], 0, Duration::from_secs(3600)).await;
    let created = h.create().await;
    let now = start_time(&created);
    let funding = crate::sandbox::funding::excluded_funding();
    let worker = worker_with_funding(&h, funding.clone());
    for _ in 0..20 {
        worker.run_one(&created.id, now).await.unwrap();
        if h.stored(&created.id).await.status == SandboxStatus::Active {
            break;
        }
    }
    let active = h.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert!(active.failure.is_none());
    assert!(active.runtime.is_some());
    let home = active.lifecycle.home.clone();
    let pause_at = now + Duration::from_secs(10);
    h.store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..20 {
        worker.run_one(&active.id, pause_at).await.unwrap();
        if h.stored(&active.id).await.status == SandboxStatus::Paused {
            break;
        }
    }
    assert_eq!(h.stored(&active.id).await.status, SandboxStatus::Paused);
    let revive_at = pause_at + Duration::from_secs(10);
    h.store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    let restarted = worker_with_funding(&h, funding.clone());
    for _ in 0..20 {
        restarted.run_one(&active.id, revive_at).await.unwrap();
        if h.stored(&active.id).await.status == SandboxStatus::Active {
            break;
        }
    }
    let resumed = h.stored(&active.id).await;
    assert_eq!(resumed.status, SandboxStatus::Active);
    assert!(resumed.failure.is_none());
    assert!(resumed.runtime.is_some());
    assert_eq!(resumed.lifecycle.home, home);
    assert_eq!(funding.renew_due(revive_at).await.unwrap(), 0);
    let events = h.store.sandbox_events(&active.id, 0, 100).await.unwrap();
    assert!(!events.iter().any(|e| matches!(&e.kind,
        SandboxEventKind::WorkStepFailed { failure, .. } if failure.detail.contains("prepaid"))));
}
