use super::reconciliation_tests::active_with_exact_image;
use super::*;
use crate::sandbox::reconciliation::SandboxReconciler;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::AgentLogKind;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

#[tokio::test]
async fn future_durable_timestamp_holds_provider_loss_for_a_fair_retry() {
    run_boundary_case(true).await;
}

#[tokio::test]
async fn newer_agent_after_pending_snapshot_holds_provider_loss() {
    run_boundary_case(false).await;
}

async fn run_boundary_case(future_durable: bool) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (mut active, _) = active_with_exact_image(&harness).await;
    if future_durable {
        let revision = active.revision;
        active.revision += 1;
        active.updated_at = rfc3339_at(SystemTime::now() + Duration::from_secs(3));
        active.validate().unwrap();
        harness
            .store
            .save_sandbox(revision, active.clone())
            .await
            .unwrap();
    } else {
        let after_durable = stamp_time(&active.updated_at) + Duration::from_millis(10);
        if let Ok(delay) = after_durable.duration_since(SystemTime::now()) {
            tokio::time::sleep(delay).await;
        }
    }
    let started = SystemTime::now();
    let mut agent = lifecycle_queue_tests::ready_agent(&active, started);
    agent.conversation.last_seq = 1;
    harness.store.create_agent(agent.clone()).await.unwrap();
    harness
        .store
        .append_agent_log(
            &agent.id,
            rfc3339_at(started),
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: agent.conversation.runtime_generation.unwrap(),
            },
        )
        .await
        .unwrap();
    let backend = BoundaryProbe {
        inner: harness.backend.clone(),
        store: harness.store.clone(),
        agent,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let adapter = Arc::new(CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec::default(),
    ));
    SandboxReconciler::new(
        harness.store.clone(),
        adapter,
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(started)
    .await
    .unwrap();
    let current = harness.stored(&active.id).await;
    assert_eq!(current.status, SandboxStatus::Active);
    assert_eq!(current.runtime, active.runtime);
}

struct BoundaryProbe {
    inner: InMemoryHostedBackend,
    store: Arc<InMemorySandboxStore>,
    agent: layer2_core::sandbox::Agent,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl HostedBackend for BoundaryProbe {
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
        request: layer3_adapter::wire::RequestFrame,
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
