use super::recovery_tests::{active_fixture, after_durable_boundary, reconcile};
use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

#[tokio::test]
async fn future_durable_timestamp_holds_retained_home_loss() {
    let (fixture, mut active, _) = active_fixture().await;
    let revision = active.revision;
    active.revision += 1;
    active.updated_at = rfc3339_at(SystemTime::now() + Duration::from_secs(3));
    active.validate().unwrap();
    fixture
        .harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    let started = SystemTime::now();
    let agent =
        lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, started, 1).await;
    let backend = RetainedBoundaryProbe {
        inner: fixture.backend.clone(),
        store: fixture.harness.store.clone(),
        agent,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let mut config = CodexSandboxSpec::default();
    config.deployment.image.uri = runtime_image();
    let adapter = Arc::new(
        CodexAdapter::with_backend(backend, config).with_home_volumes(fixture.homes.clone()),
    );
    SandboxReconciler::new(
        fixture.harness.store.clone(),
        adapter,
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    )
    .initialize(started)
    .await
    .unwrap();
    let current = fixture.harness.stored(&active.id).await;
    assert_eq!(current.status, SandboxStatus::Active);
    assert_eq!(current.runtime, active.runtime);
    assert_eq!(current.lifecycle.home, active.lifecycle.home);
}

#[tokio::test]
async fn newer_agent_after_pending_snapshot_holds_retained_home_loss() {
    let (fixture, active, _) = active_fixture().await;
    let started = after_durable_boundary(&active).await;
    let agent =
        lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, started, 1).await;
    let backend = RetainedBoundaryProbe {
        inner: fixture.backend.clone(),
        store: fixture.harness.store.clone(),
        agent,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let mut config = CodexSandboxSpec::default();
    config.deployment.image.uri = runtime_image();
    let adapter = Arc::new(
        CodexAdapter::with_backend(backend, config).with_home_volumes(fixture.homes.clone()),
    );
    SandboxReconciler::new(
        fixture.harness.store.clone(),
        adapter,
        Arc::new(fixture.harness.policy.clone()),
        "test".into(),
    )
    .initialize(started)
    .await
    .unwrap();
    let current = fixture.harness.stored(&active.id).await;
    assert_eq!(current.status, SandboxStatus::Active);
    assert_eq!(current.runtime, active.runtime);
    assert_eq!(current.lifecycle.home, active.lifecycle.home);
}

#[tokio::test]
async fn fresh_retained_home_loss_still_recovers_after_old_boundary() {
    let (fixture, active, now) = active_fixture().await;
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    let after_durable = stamp_time(&active.updated_at).max(stamp_time(&agent.updated_at))
        + Duration::from_millis(10);
    if let Ok(delay) = after_durable.duration_since(SystemTime::now()) {
        tokio::time::sleep(delay).await;
    }
    fixture.backend.restart_process().await;
    reconcile(&fixture, SystemTime::UNIX_EPOCH).await;
    let current = fixture.harness.stored(&active.id).await;
    assert_eq!(current.status, SandboxStatus::Reviving);
    assert_eq!(current.lifecycle.home, active.lifecycle.home);
    assert_eq!(
        current.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    assert!(current.runtime.is_none());
    let recovered = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.id, agent.id);
    assert_eq!(
        recovered.conversation.conversation_id,
        agent.conversation.conversation_id
    );
}

struct RetainedBoundaryProbe {
    inner: NativeBackend,
    store: Arc<InMemorySandboxStore>,
    agent: layer2_core::sandbox::Agent,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl HostedBackend for RetainedBoundaryProbe {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }

    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        let nth = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut snapshot = self.inner.inventory().await?;
        for observed in &mut snapshot {
            observed.lifecycle.state = "pending".into();
        }
        if nth == 2 {
            tokio::time::sleep(Duration::from_millis(1_100)).await;
            let mut newer = self.agent.clone();
            newer.revision += 1;
            newer.updated_at = rfc3339_at(SystemTime::now());
            self.store
                .save_agent(self.agent.revision, newer)
                .await
                .unwrap();
        }
        Ok(snapshot)
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
