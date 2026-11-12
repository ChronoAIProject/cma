use super::*;

use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::AgentLogKind;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use tokio::sync::mpsc;

use crate::sandbox::reconciliation::SandboxReconciler;
use crate::sandbox::runtime_mapping::sandbox_spec;

fn reconciler(harness: &Harness, adapter: Arc<dyn SandboxAdapter>) -> SandboxReconciler {
    SandboxReconciler::new(
        harness.store.clone(),
        adapter,
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
}

fn fresh_adapter(harness: &Harness, image: Option<&str>) -> Arc<dyn SandboxAdapter> {
    let mut spec = CodexSandboxSpec::default();
    if let Some(image) = image {
        spec.deployment.image.uri = image.into();
    }
    Arc::new(CodexAdapter::with_backend(harness.backend.clone(), spec))
}

pub(super) async fn active_with_exact_image(harness: &Harness) -> (Sandbox, SystemTime) {
    let created = harness.create().await;
    let now = start_time(&created);
    let mut active = harness.drive_active(&created.id, now).await;
    let revision = active.revision;
    active.runtime.as_mut().unwrap().runtime_version =
        CodexSandboxSpec::default().deployment.image.uri;
    active.revision += 1;
    harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    (active, now)
}

#[tokio::test]
async fn startup_and_reconciliation_retain_the_persisted_image_across_upgrade_and_rollback() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let adapter = fresh_adapter(&harness, Some("registry.example/runtime:new"));
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    assert!(adapter.resources(&runtime).await.is_err());
    let reconciler = reconciler(&harness, adapter.clone());
    reconciler.initialize(now).await.unwrap();
    assert_eq!(harness.stored(&active.id).await, active);
    adapter.resources(&runtime).await.unwrap();

    let before_requests = harness.backend.requests().await;
    for tick in 0..4 {
        reconciler
            .run_due(now + Duration::from_secs(tick * 15))
            .await
            .unwrap();
        assert_eq!(harness.stored(&active.id).await, active);
        assert_eq!(adapter.runtime_inventory().await.unwrap().len(), 1);
    }
    assert_eq!(harness.backend.requests().await, before_requests);

    let rollback = fresh_adapter(&harness, Some("registry.example/runtime:rollback"));
    self::reconciler(&harness, rollback.clone())
        .run_due(now + Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(harness.stored(&active.id).await, active);
    assert_eq!(harness.backend.requests().await, before_requests);
    rollback.resources(&runtime).await.unwrap();
}

#[tokio::test]
async fn new_allocations_use_the_new_image_while_both_cohorts_survive_rollback() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (retained, now) = active_with_exact_image(&harness).await;
    let old_runtime = RuntimeHandle(retained.runtime.as_ref().unwrap().runtime_id.clone());
    let old_snapshot = harness.backend.snapshot(&old_runtime).await.unwrap();
    let new_image = "registry.example/runtime:new";
    let upgraded = fresh_adapter(&harness, Some(new_image));
    reconciler(&harness, upgraded.clone())
        .initialize(now)
        .await
        .unwrap();

    let created = harness.create().await;
    let worker = harness.worker_with_runtime_image("new-worker", upgraded.clone(), new_image);
    for _ in 0..8 {
        worker.run_one(&created.id, now).await.unwrap();
    }
    let allocated = harness.stored(&created.id).await;
    assert_eq!(allocated.status, SandboxStatus::Active);
    assert_eq!(
        allocated.runtime.as_ref().unwrap().runtime_version,
        new_image
    );
    let new_runtime = RuntimeHandle(allocated.runtime.as_ref().unwrap().runtime_id.clone());
    assert_ne!(new_runtime, old_runtime);
    let new_snapshot = harness.backend.snapshot(&new_runtime).await.unwrap();
    assert_eq!(new_snapshot.request.image.uri, new_image);
    assert_ne!(old_snapshot.request.image.uri, new_image);

    for adapter in [upgraded, fresh_adapter(&harness, None)] {
        let before_requests = harness.backend.requests().await;
        reconciler(&harness, adapter.clone())
            .run_due(now + Duration::from_secs(15))
            .await
            .unwrap();
        assert_eq!(harness.stored(&retained.id).await, retained);
        assert_eq!(harness.stored(&created.id).await, allocated);
        assert_eq!(
            harness.backend.snapshot(&old_runtime).await.unwrap(),
            old_snapshot
        );
        assert_eq!(
            harness.backend.snapshot(&new_runtime).await.unwrap(),
            new_snapshot
        );
        assert_eq!(harness.backend.requests().await, before_requests);
        assert_eq!(adapter.runtime_inventory().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn startup_during_install_continues_on_the_adopted_old_image() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let mut installing = harness.stored(&created.id).await;
    while installing.runtime.is_none() {
        harness.run(&created.id, now).await;
        installing = harness.stored(&created.id).await;
    }
    assert_eq!(installing.work.step, SandboxWorkStep::RunInstall);
    let revision = installing.revision;
    installing.runtime.as_mut().unwrap().runtime_version =
        CodexSandboxSpec::default().deployment.image.uri;
    installing.revision += 1;
    harness
        .store
        .save_sandbox(revision, installing.clone())
        .await
        .unwrap();
    let adapter = fresh_adapter(&harness, Some("registry.example/runtime:new"));
    reconciler(&harness, adapter.clone())
        .initialize(now)
        .await
        .unwrap();
    let worker = harness.worker_with_adapter("replacement-worker", adapter.clone());
    for _ in 0..4 {
        worker.run_one(&created.id, now).await.unwrap();
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.runtime, installing.runtime);
    assert_eq!(adapter.runtime_inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn startup_missing_runtime_without_checkpoint_fails_atomically() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, _initial_now) = active_with_exact_image(&harness).await;
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    harness.adapter.teardown(&runtime).await.unwrap();
    let durable = stamp_time(&active.updated_at);
    if let Ok(delay) = durable
        .duration_since(SystemTime::now())
        .map(|delay| delay + Duration::from_millis(10))
    {
        tokio::time::sleep(delay).await;
    }
    let now = SystemTime::now();
    let reconciler = reconciler(&harness, fresh_adapter(&harness, None));
    reconciler.initialize(now).await.unwrap();
    let failed = harness.stored(&active.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.failure.as_ref().unwrap().class, "runtime_lost");
    assert!(failed.runtime.is_none());
    reconciler.initialize(now).await.unwrap();
    assert_eq!(harness.stored(&active.id).await, failed);
}

#[tokio::test]
async fn continuous_reconciliation_uses_live_inactivity_policy() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let mut agent = lifecycle_queue_tests::ready_agent(&active, now);
    agent.conversation.last_seq = 1;
    harness.store.create_agent(agent.clone()).await.unwrap();
    harness
        .store
        .append_agent_log(
            &agent.id,
            rfc3339_at(now),
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: agent.conversation.runtime_generation.unwrap(),
            },
        )
        .await
        .unwrap();
    let policy = Arc::new(MutablePolicy(Mutex::new(harness.policy.clone())));
    let reconciler = SandboxReconciler::new(
        harness.store.clone(),
        fresh_adapter(&harness, None),
        policy.clone(),
        "test".into(),
    );
    reconciler
        .run_due(now + Duration::from_secs(800))
        .await
        .unwrap();
    assert_eq!(
        harness.stored(&active.id).await.status,
        SandboxStatus::Active
    );
    policy.0.lock().unwrap().inactivity_pause_after_seconds = 600;
    reconciler
        .run_due(now + Duration::from_secs(800))
        .await
        .unwrap();
    let pausing = harness.stored(&active.id).await;
    assert_eq!(pausing.status, SandboxStatus::Pausing);
    assert!(pausing
        .lifecycle
        .paused_notice
        .unwrap()
        .contains("10 minutes"));
}

#[tokio::test]
async fn startup_reclaims_only_provider_rows_without_a_durable_sandbox() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let protected = harness
        .adapter
        .provision(&sandbox_spec(&created, "test"))
        .await
        .unwrap();
    let mut orphan_spec = sandbox_spec(&created, "test");
    orphan_spec.sandbox_id = "sbx_orphan".into();
    let orphan = harness.adapter.provision(&orphan_spec).await.unwrap();
    reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(start_time(&created))
        .await
        .unwrap();
    assert!(harness.backend.snapshot(&protected).await.is_ok());
    assert!(harness.backend.snapshot(&orphan).await.is_err());
    assert_eq!(harness.stored(&created.id).await, created);
}

#[tokio::test]
async fn obsolete_provider_generation_is_reclaimed_below_durable_high_water() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (mut active, now) = active_with_exact_image(&harness).await;
    let old = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let replacement = harness
        .adapter
        .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
            2,
            sandbox_spec(&active, "test"),
        ))
        .await
        .unwrap();
    let revision = active.revision;
    active.runtime.as_mut().unwrap().runtime_id = replacement.0.clone();
    active.runtime.as_mut().unwrap().provision_generation = Some(2);
    active.lifecycle.runtime_generation_high_water = 2;
    active.revision += 1;
    harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    let adapter = fresh_adapter(&harness, None);
    reconciler(&harness, adapter).initialize(now).await.unwrap();
    assert!(harness.backend.snapshot(&old).await.is_err());
    assert!(harness.backend.snapshot(&replacement).await.is_ok());
    assert_eq!(harness.stored(&active.id).await, active);
}

#[tokio::test]
async fn newer_unbound_generation_is_not_reclaimed_during_provisioning() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let pending = harness
        .adapter
        .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
            2,
            sandbox_spec(&active, "test"),
        ))
        .await
        .unwrap();
    reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(now)
        .await
        .unwrap();
    assert!(harness.backend.snapshot(&pending).await.is_ok());
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 2);
    assert_eq!(harness.stored(&active.id).await, active);
}

#[tokio::test]
async fn startup_completes_global_pagination_before_reclaiming() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let mut last = created.clone();
    for index in 0..205 {
        let mut sandbox = created.clone();
        sandbox.id = SandboxId(format!("sbx_paged_{index:03}"));
        sandbox.name = format!("paged-{index}");
        harness.store.create_sandbox(sandbox.clone()).await.unwrap();
        last = sandbox;
    }
    let protected = harness
        .adapter
        .provision(&sandbox_spec(&last, "test"))
        .await
        .unwrap();
    reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(start_time(&created))
        .await
        .unwrap();
    assert!(harness.backend.snapshot(&protected).await.is_ok());
    assert_eq!(harness.stored(&last.id).await, last);
}

#[tokio::test]
async fn settled_unbound_parents_reclaim_equal_generation_without_touching_newer_allocations() {
    for status in [
        SandboxStatus::Paused,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ] {
        let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
        let (mut current, now) = active_with_exact_image(&harness).await;
        let obsolete = RuntimeHandle(current.runtime.as_ref().unwrap().runtime_id.clone());
        let newer = harness
            .adapter
            .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
                2,
                sandbox_spec(&current, "test"),
            ))
            .await
            .unwrap();
        let revision = current.revision;
        current.runtime = None;
        current.status = status;
        current
            .work
            .schedule(SandboxWorkStep::None, rfc3339_at(now));
        current.lifecycle.checkpoints.latest = Some(
            layer2_core::sandbox::SandboxCheckpointRef::seal(
                &layer3_adapter::CheckpointReceipt {
                    epoch: 1,
                    object_reference: "checkpoint/settled".into(),
                    sha256: "a".repeat(64),
                    writers: layer3_adapter::WriterState::Resumed,
                },
                &[],
                rfc3339_at(now),
            )
            .unwrap(),
        );
        if status == SandboxStatus::Retired {
            current.retired_at = Some(rfc3339_at(now));
        }
        current.revision += 1;
        harness
            .store
            .save_sandbox(revision, current.clone())
            .await
            .unwrap();
        reconciler(&harness, fresh_adapter(&harness, None))
            .initialize(now)
            .await
            .unwrap();
        assert!(
            harness.backend.snapshot(&obsolete).await.is_err(),
            "{status:?}"
        );
        assert!(harness.backend.snapshot(&newer).await.is_ok(), "{status:?}");
        assert_eq!(harness.stored(&current.id).await, current);
    }
}

#[tokio::test]
async fn equal_generation_remains_protected_before_initial_binding_publication() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let mut created = harness.create().await;
    let revision = created.revision;
    created.lifecycle.runtime_generation_high_water = 1;
    created.revision += 1;
    harness
        .store
        .save_sandbox(revision, created.clone())
        .await
        .unwrap();
    let pending = harness
        .adapter
        .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
            1,
            sandbox_spec(&created, "test"),
        ))
        .await
        .unwrap();
    reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(start_time(&created))
        .await
        .unwrap();
    assert!(harness.backend.snapshot(&pending).await.is_ok());
    assert_eq!(harness.stored(&created.id).await, created);
}

#[tokio::test]
async fn stale_inventory_is_refetched_before_declaring_a_bound_runtime_missing() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let backend = InventoryProbe::new(harness.backend.clone(), vec![Ok(Vec::new())]);
    let calls = backend.calls.clone();
    let adapter: Arc<dyn SandboxAdapter> = Arc::new(CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec::default(),
    ));
    reconciler(&harness, adapter.clone())
        .initialize(now)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(harness.stored(&active.id).await, active);
    adapter
        .resources(&RuntimeHandle(active.runtime.unwrap().runtime_id))
        .await
        .unwrap();
}

#[tokio::test]
async fn incomplete_provider_inventory_cannot_recover_or_reclaim_anything() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let backend = InventoryProbe::new(
        harness.backend.clone(),
        vec![Err(layer3_adapter::AdapterError::Unavailable(
            "inventory page unavailable".into(),
        ))],
    );
    let adapter = Arc::new(CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec::default(),
    ));
    assert!(reconciler(&harness, adapter).initialize(now).await.is_err());
    assert_eq!(harness.stored(&active.id).await, active);
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn incomplete_database_authority_cannot_reclaim_provider_only_runtimes() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let mut spec = sandbox_spec(&created, "test");
    spec.sandbox_id = "sbx_orphan".into();
    let orphan = harness.adapter.provision(&spec).await.unwrap();
    let mut corrupt = created.clone();
    corrupt.revision += 1;
    corrupt.work.next_fence = 0;
    harness
        .store
        .save_sandbox(created.revision, corrupt.clone())
        .await
        .unwrap();
    assert!(reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(start_time(&created))
        .await
        .is_err());
    assert!(harness.backend.snapshot(&orphan).await.is_ok());
    assert_eq!(harness.stored(&created.id).await, corrupt);
}

#[tokio::test]
async fn bound_runtime_with_conflicting_sandbox_identity_is_not_adopted_or_reclaimed() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (mut active, now) = active_with_exact_image(&harness).await;
    let mut spec = sandbox_spec(&active, "test");
    spec.sandbox_id = "sbx_other".into();
    let other = harness.adapter.provision(&spec).await.unwrap();
    let revision = active.revision;
    active.runtime.as_mut().unwrap().runtime_id = other.0.clone();
    active.revision += 1;
    harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    assert!(reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(now)
        .await
        .is_err());
    assert_eq!(harness.stored(&active.id).await, active);
    assert!(harness.backend.snapshot(&other).await.is_ok());
}

#[tokio::test]
async fn periodic_reconciliation_exits_on_shutdown() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (shutdown, signal) = tokio::sync::watch::channel(false);
    let activity = crate::worker_supervision::Workers::new().get("sandbox_reconciliation");
    let task =
        reconciler(&harness, fresh_adapter(&harness, None)).spawn_supervised(signal, activity);
    shutdown.send(true).unwrap();
    task.await.unwrap();
}

pub(super) struct InventoryProbe {
    inner: InMemoryHostedBackend,
    replies: Mutex<VecDeque<layer3_adapter::Result<Vec<RuntimeObservation>>>>,
    calls: Arc<AtomicUsize>,
}

impl InventoryProbe {
    pub(super) fn new(
        inner: InMemoryHostedBackend,
        replies: Vec<layer3_adapter::Result<Vec<RuntimeObservation>>>,
    ) -> Self {
        Self {
            inner,
            replies: Mutex::new(replies.into()),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for InventoryProbe {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }

    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self.replies.lock().unwrap().pop_front();
        match reply {
            Some(reply) => reply,
            None => self.inner.inventory().await,
        }
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.inner.adopt(handle, request).await
    }

    async fn reclaim(&self, observation: &RuntimeObservation) -> layer3_adapter::Result<()> {
        self.inner.reclaim(observation).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        self.inner.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.inner
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.inner.teardown(handle).await
    }
}
