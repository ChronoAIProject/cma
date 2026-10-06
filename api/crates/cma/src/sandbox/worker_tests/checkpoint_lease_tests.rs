use super::*;
use layer2_core::sandbox::store::{
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard, SandboxReconciliationStore,
};
use layer2_core::sandbox::{SandboxCheckpointCapturePhase, SANDBOX_CHECKPOINT_INTERVAL_SECONDS};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};

use crate::sandbox::checkpoint_maintenance::SandboxCheckpointMaintenance;

struct HeldReply {
    backend: InMemoryHostedBackend,
    gate_once: AtomicBool,
    entered: Notify,
    release: Notify,
    store: Arc<InMemorySandboxStore>,
    claims: Mutex<Vec<(SandboxId, String)>>,
}

#[async_trait::async_trait]
impl HostedBackend for HeldReply {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.backend.provision(request).await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if matches!(request, RequestFrame::CheckpointForTeardown(_)) {
            let page = self
                .store
                .sandbox_runtime_authority(&PageRequest::new(200, None).unwrap())
                .await
                .unwrap();
            let sandbox = page
                .items
                .into_iter()
                .find(|sandbox| {
                    sandbox
                        .runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.runtime_id == handle.0)
                })
                .unwrap();
            if let Some(capture) = sandbox.lifecycle.checkpoint_capture {
                self.claims
                    .lock()
                    .unwrap()
                    .push((sandbox.id, capture.work.lease.unwrap().expires_at));
            }
        }
        let gate = matches!(request, RequestFrame::CheckpointForTeardown(_))
            && self.gate_once.swap(false, Ordering::SeqCst);
        let result = self.backend.exchange(handle, request).await;
        if gate {
            self.entered.notify_one();
            self.release.notified().await;
        }
        result
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

async fn fixture() -> (
    Harness,
    Sandbox,
    SystemTime,
    Arc<HeldReply>,
    Arc<SandboxCheckpointMaintenance>,
) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let gate = Arc::new(HeldReply {
        backend: harness.backend.clone(),
        gate_once: AtomicBool::new(true),
        entered: Notify::new(),
        release: Notify::new(),
        store: harness.store.clone(),
        claims: Mutex::new(Vec::new()),
    });
    let adapter = Arc::new(CodexAdapter::new(gate.clone(), CodexSandboxSpec::default()));
    let worker = SandboxCheckpointMaintenance::new(
        harness.store.clone(),
        adapter,
        "test".into(),
        "renewing".into(),
    );
    (
        harness,
        active,
        now + Duration::from_secs(SANDBOX_CHECKPOINT_INTERVAL_SECONDS + 1),
        gate,
        worker,
    )
}

fn token(sandbox: &Sandbox) -> String {
    sandbox
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .authority
        .writer_resume_token
        .clone()
        .unwrap()
}

async fn minute() {
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

#[tokio::test]
async fn foreground_checkpoint_revision_conflict_retries_without_waiting_for_capture_lease() {
    let (harness, active, now, gate, _) = fixture().await;
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    let adapter = Arc::new(CodexAdapter::new(gate.clone(), CodexSandboxSpec::default()));
    let worker = harness.worker_with_adapter("foreground", adapter);
    let task = tokio::spawn({
        let worker = worker.clone();
        let id = active.id.clone();
        async move { worker.run_one(&id, now).await }
    });
    gate.entered.notified().await;
    let mut changed = harness.stored(&active.id).await;
    let operation = changed.lifecycle.operation.clone();
    let revision = changed.revision;
    changed.revision += 1;
    changed.updated_at = rfc3339_at(now + Duration::from_secs(1));
    harness.store.save_sandbox(revision, changed).await.unwrap();
    gate.release.notify_one();
    assert!(task.await.unwrap().unwrap());
    let retry = harness.stored(&active.id).await;
    assert!(retry.work.lease.is_none());
    assert_eq!(retry.work.attempts, 1);
    assert_eq!(retry.lifecycle.operation, operation);
    let retry_at = now + Duration::from_secs(6);
    for _ in 0..4 {
        worker.run_one(&active.id, retry_at).await.unwrap();
    }
    let stopped = harness.stored(&active.id).await;
    assert_eq!(stopped.status, SandboxStatus::Paused);
    assert!(stopped.runtime.is_none());
    assert!(stopped.lifecycle.checkpoints.latest.is_some());
}

#[tokio::test]
async fn foreground_checkpoint_conflict_cannot_release_a_successor_worker_lease() {
    let (harness, active, now, gate, _) = fixture().await;
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    let adapter = Arc::new(CodexAdapter::new(gate.clone(), CodexSandboxSpec::default()));
    let worker = harness.worker_with_adapter("foreground", adapter);
    let task = tokio::spawn({
        let id = active.id.clone();
        async move { worker.run_one(&id, now).await }
    });
    gate.entered.notified().await;
    let claimed = harness.stored(&active.id).await;
    let expiry = claimed.work.lease.as_ref().unwrap().expires_at.clone();
    let successor = harness
        .store
        .claim_sandbox_work(
            &active.id,
            "successor",
            expiry,
            rfc3339_at(now + Duration::from_secs(7_200)),
        )
        .await
        .unwrap()
        .unwrap();
    gate.release.notify_one();
    assert_eq!(task.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(harness.stored(&active.id).await, successor);
}

#[tokio::test(start_paused = true)]
async fn long_capture_renews_one_fence_and_publishes_with_fresh_completion_time() {
    let (harness, active, now, gate, worker) = fixture().await;
    worker.run_once(now).await.unwrap();
    let task = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run_once(now).await }
    });
    gate.entered.notified().await;
    let initial = harness.stored(&active.id).await;
    let authority = token(&initial);
    let fence = initial
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence;
    for elapsed in 1..=6 {
        minute().await;
        let current = harness.stored(&active.id).await;
        let capture = current.lifecycle.checkpoint_capture.as_ref().unwrap();
        let lease = capture.work.lease.as_ref().unwrap();
        assert_eq!(lease.fence, fence);
        assert_eq!(token(&current), authority);
        assert_eq!(capture.work.attempts, 0);
        assert!(lease.expires_at > rfc3339_at(now + Duration::from_secs(elapsed * 60)));
    }
    let competitor = SandboxCheckpointMaintenance::new(
        harness.store.clone(),
        harness.adapter.clone(),
        "test".into(),
        "competitor".into(),
    );
    assert_eq!(
        competitor
            .run_once(now + Duration::from_secs(360))
            .await
            .unwrap(),
        0
    );
    gate.release.notify_one();
    task.await.unwrap().unwrap();
    let published = harness.stored(&active.id).await;
    assert_eq!(
        published
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .phase,
        SandboxCheckpointCapturePhase::ResumeWriters
    );
    assert_eq!(
        published
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .unwrap()
            .sealed_at,
        rfc3339_at(now + Duration::from_secs(360))
    );
    worker
        .run_once(now + Duration::from_secs(360))
        .await
        .unwrap();
    assert!(harness
        .stored(&active.id)
        .await
        .lifecycle
        .checkpoint_capture
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn stalled_capture_reaches_deadline_and_preserves_exact_resume_debt() {
    let (harness, active, now, gate, worker) = fixture().await;
    worker.run_once(now).await.unwrap();
    let task = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run_once(now).await }
    });
    gate.entered.notified().await;
    let authority = token(&harness.stored(&active.id).await);
    for _ in 0..30 {
        minute().await;
    }
    task.await.unwrap().unwrap();
    let debt = harness.stored(&active.id).await;
    assert_eq!(token(&debt), authority);
    assert_eq!(
        debt.lifecycle.checkpoint_capture.as_ref().unwrap().phase,
        SandboxCheckpointCapturePhase::ResumeWriters
    );
    assert!(debt.lifecycle.checkpoints.latest.is_none());
    worker
        .run_once(now + Duration::from_secs(1800))
        .await
        .unwrap();
    assert!(harness
        .stored(&active.id)
        .await
        .lifecycle
        .checkpoint_capture
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn takeover_fences_a_late_capture_result_without_erasing_successor_resume_debt() {
    takeover_during_capture(true).await;
}

#[tokio::test(start_paused = true)]
async fn failed_renewal_cancels_stalled_io_without_erasing_successor_resume_debt() {
    takeover_during_capture(false).await;
}

async fn takeover_during_capture(release_reply: bool) {
    let (harness, active, now, gate, worker) = fixture().await;
    worker.run_once(now).await.unwrap();
    let task = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run_once(now).await }
    });
    gate.entered.notified().await;
    let old = harness.stored(&active.id).await;
    let mut successor = old.clone();
    successor
        .claim_checkpoint_capture(
            "successor".into(),
            rfc3339_at(now + Duration::from_secs(301)),
            rfc3339_at(now + Duration::from_secs(601)),
        )
        .unwrap();
    harness
        .store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&old),
            sandbox: successor.clone(),
            agents: Vec::new(),
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: rfc3339_at(now + Duration::from_secs(301)),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
    if release_reply {
        gate.release.notify_one();
    } else {
        minute().await;
    }
    task.await.unwrap().unwrap();
    assert_eq!(harness.stored(&active.id).await, successor);
    assert!(successor.lifecycle.checkpoints.latest.is_none());
}

#[tokio::test(start_paused = true)]
async fn later_batch_claims_and_checkpoint_timestamps_use_fresh_time() {
    let (harness, active, now, gate, worker) = fixture().await;
    let created = harness.create().await;
    let second = harness.drive_active(&created.id, now).await;
    let now = now + Duration::from_secs(SANDBOX_CHECKPOINT_INTERVAL_SECONDS);
    assert_eq!(worker.run_once(now).await.unwrap(), 2);
    let task = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run_once(now).await }
    });
    gate.entered.notified().await;
    for _ in 0..6 {
        minute().await;
    }
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap(), 2);
    let claims = gate.claims.lock().unwrap().clone();
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[0].0, claims[1].0);
    assert_eq!(claims[0].1, rfc3339_at(now + Duration::from_secs(300)));
    assert_eq!(claims[1].1, rfc3339_at(now + Duration::from_secs(660)));
    for id in [active.id, second.id] {
        let sandbox = harness.stored(&id).await;
        let stamp = rfc3339_at(now + Duration::from_secs(360));
        assert_eq!(sandbox.updated_at, stamp);
        assert_eq!(
            sandbox.lifecycle.checkpoints.latest.unwrap().sealed_at,
            stamp
        );
    }
}

async fn claim_pause(harness: &Harness, active: &Sandbox, now: SystemTime) -> Sandbox {
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    harness
        .store
        .claim_sandbox_work(
            &active.id,
            "worker-a",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(1860)),
        )
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn lifecycle_capture_deadline_retains_authority_and_replays_the_hold() {
    let (harness, active, now, gate, _) = fixture().await;
    let claimed = claim_pause(&harness, &active, now).await;
    let authority = claimed
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .authority()
        .clone();
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let worker = harness.worker_with_adapter(
        "worker-a",
        Arc::new(CodexAdapter::new(gate.clone(), CodexSandboxSpec::default())),
    );
    let task = tokio::spawn(async move { worker.execute_claimed_for_test(claimed, now).await });
    gate.entered.notified().await;
    tokio::time::advance(Duration::from_secs(1800)).await;
    task.await.unwrap().unwrap();
    let retry = harness.stored(&active.id).await;
    assert!(retry.lifecycle.checkpoints.latest.is_none());
    assert_eq!(
        retry.lifecycle.operation.as_ref().unwrap().authority(),
        &authority
    );
    assert_eq!(retry.work.attempts, 1);
    assert_eq!(
        retry.work.ready_at,
        rfc3339_at(now + Duration::from_secs(1805))
    );
    assert!(
        harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    assert!(!harness
        .backend
        .requests()
        .await
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::ResumeCheckpointWriters(_))));
    harness
        .run(&active.id, now + Duration::from_secs(1805))
        .await;
    let published = harness.stored(&active.id).await;
    assert!(published.lifecycle.checkpoints.latest.is_some());
    assert_eq!(
        published.lifecycle.operation.as_ref().unwrap().authority(),
        &authority
    );
}

#[tokio::test(start_paused = true)]
async fn lifecycle_publication_committed_before_timeout_keeps_its_writer_hold() {
    let (harness, active, now, _, _) = fixture().await;
    let claimed = claim_pause(&harness, &active, now).await;
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let committed = Arc::new(Notify::new());
    let task = tokio::spawn({
        let worker = harness.worker.clone();
        let committed = committed.clone();
        async move {
            worker
                .checkpoint_capture_deadline(&claimed, now, tokio::time::Instant::now(), async {
                    worker
                        .execute_claimed_for_test(claimed.clone(), now)
                        .await?;
                    committed.notify_one();
                    std::future::pending().await
                })
                .await
        }
    });
    committed.notified().await;
    let published = harness.stored(&active.id).await;
    assert!(published.lifecycle.checkpoints.latest.is_some());
    tokio::time::advance(Duration::from_secs(1800)).await;
    assert_eq!(task.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(harness.stored(&active.id).await, published);
    assert!(
        harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held
    );
    assert!(!harness
        .backend
        .requests()
        .await
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::ResumeCheckpointWriters(_))));
}
