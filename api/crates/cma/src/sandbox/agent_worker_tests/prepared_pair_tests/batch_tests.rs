use super::*;
use layer3_adapter::*;
use std::sync::atomic::AtomicBool;

struct ObservedAdapter {
    inner: Arc<dyn SandboxAdapter>,
    store: Arc<InMemorySandboxStore>,
    agent_id: AgentId,
    support: AtomicBool,
    inspections: AtomicUsize,
    batches: AtomicUsize,
    individual: AtomicUsize,
    lose_response: AtomicBool,
    refuse_inspection: AtomicBool,
}

#[async_trait::async_trait]
impl SandboxAdapter for ObservedAdapter {
    async fn inspect_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> layer3_adapter::Result<Option<RuntimeInspection>> {
        self.inspections.fetch_add(1, Ordering::SeqCst);
        if self.refuse_inspection.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fresh process proof unavailable".into(),
            ));
        }
        let mut inspected = self.inner.inspect_runtime(request).await?.unwrap();
        // This port fixture supplies the observed capability for the exact
        // in-memory runtime. Production obtains it from authenticated status.
        inspected.status = Some(
            serde_json::from_value(serde_json::json!({
                "sandbox_id":request.spec.sandbox_id, "generation":request.generation,
                "home_identity":"a".repeat(43), "boot_id":"a".repeat(32),
                "installation_ready":true, "credential_ready":true,
                "restore_pending":false, "agent_processes":1,
                "agent_resource_preparation":self.support.load(Ordering::SeqCst)
            }))
            .unwrap(),
        );
        Ok(Some(inspected))
    }

    async fn prepare_agent_resources(
        &self,
        handle: &RuntimeHandle,
        fence: u64,
        worktree: &WorktreeRequest,
        home: &AgentHomeRequest,
    ) -> layer3_adapter::Result<WorktreeReceipt> {
        self.batches.fetch_add(1, Ordering::SeqCst);
        let agent = self.store.get_agent(&self.agent_id).await.unwrap().unwrap();
        assert_eq!(
            agent.work.cleanup_debt.len(),
            2,
            "both intents precede either effect"
        );
        assert!(agent.work.receipts.is_empty());
        assert_eq!(agent.work.lease.as_ref().unwrap().fence, fence);
        let receipt = self.inner.add_worktree(handle, fence, worktree).await?;
        self.inner
            .materialize_agent_home(handle, fence, home)
            .await?;
        if self.lose_response.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "response lost after both effects".into(),
            ));
        }
        Ok(receipt)
    }

    fn capabilities(&self) -> AdapterCapabilities {
        self.inner.capabilities()
    }

    async fn provision(&self, spec: &SandboxSpec) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(spec).await
    }

    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> layer3_adapter::Result<InstallReceipt> {
        self.inner.run_install(handle, request).await
    }

    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> layer3_adapter::Result<()> {
        self.inner.deliver_credential(handle, frame).await
    }

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> layer3_adapter::Result<WorkspaceReceipt> {
        self.inner.prepare_workspace(handle, request).await
    }

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> layer3_adapter::Result<SetupReceipt> {
        self.inner.run_workspace_setup(handle, request).await
    }

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> layer3_adapter::Result<()> {
        self.inner.remove_workspace(handle, request).await
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> layer3_adapter::Result<WorktreeReceipt> {
        self.individual.fetch_add(1, Ordering::SeqCst);
        self.inner
            .add_worktree(handle, authority_fence, request)
            .await
    }

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> layer3_adapter::Result<()> {
        self.inner
            .remove_worktree(handle, authority_fence, request)
            .await
    }

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> layer3_adapter::Result<()> {
        self.individual.fetch_add(1, Ordering::SeqCst);
        self.inner
            .materialize_agent_home(handle, authority_fence, request)
            .await
    }

    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> layer3_adapter::Result<()> {
        self.inner
            .remove_agent_home(handle, authority_fence, request)
            .await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<ConversationChannel> {
        self.inner
            .open_agent(handle, authority_fence, request, updates)
            .await
    }

    async fn reconnect_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<ConversationChannel> {
        self.inner
            .reconnect_agent(handle, authority_fence, request, conversation_id, updates)
            .await
    }

    async fn close_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentCloseRequest,
    ) -> layer3_adapter::Result<()> {
        self.inner
            .close_agent(handle, authority_fence, request)
            .await
    }

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> layer3_adapter::Result<ConversationSnapshot> {
        self.inner.capture_agent(handle, agent).await
    }

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> layer3_adapter::Result<CheckpointReceipt> {
        self.inner.checkpoint(handle, request).await
    }

    async fn restore(
        &self,
        handle: &RuntimeHandle,
        request: &RestoreRequest,
    ) -> layer3_adapter::Result<()> {
        self.inner.restore(handle, request).await
    }

    async fn resources(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<ResourceReading> {
        self.inner.resources(handle).await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.inner.teardown(handle).await
    }
}

fn use_adapter(harness: &mut Harness, agent: &Agent, supported: bool) -> Arc<ObservedAdapter> {
    let adapter = Arc::new(ObservedAdapter {
        inner: harness.adapter.clone(),
        store: harness.store.clone(),
        agent_id: agent.id.clone(),
        support: AtomicBool::new(supported),
        inspections: AtomicUsize::new(0),
        batches: AtomicUsize::new(0),
        individual: AtomicUsize::new(0),
        lose_response: AtomicBool::new(false),
        refuse_inspection: AtomicBool::new(false),
    });
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.lease_duration = Duration::from_secs(30);
    config.retry_delay = Duration::from_secs(1);
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        config,
    );
    adapter
}

#[tokio::test]
async fn prepared_batch_uses_only_freshly_advertised_capability_and_keeps_both_receipts() {
    for supported in [false, true] {
        let (mut harness, agent) = prepared().await;
        let adapter = use_adapter(&mut harness, &agent, supported);
        let claimed = claim(&harness, &agent, harness.now).await;
        harness
            .worker
            .replay_prepared_for_test(claimed, harness.now)
            .await
            .unwrap();
        assert_eq!(adapter.inspections.load(Ordering::SeqCst), 1);
        assert_eq!(
            adapter.batches.load(Ordering::SeqCst),
            usize::from(supported)
        );
        assert_eq!(
            adapter.individual.load(Ordering::SeqCst),
            if supported { 0 } else { 2 }
        );
        let completed = harness.stored(&agent.id).await;
        assert_eq!(completed.work.step, AgentWorkStep::OpenConversation);
        assert_eq!(completed.work.receipts.len(), 2);
        assert!(completed.work.cleanup_debt.is_empty());
    }
}

#[tokio::test]
async fn prepared_batch_lost_reply_keeps_debts_without_fallback_and_reinspects_on_retry() {
    for retained_support in [false, true] {
        let (mut harness, agent) = prepared().await;
        let adapter = use_adapter(&mut harness, &agent, true);
        adapter.lose_response.store(true, Ordering::SeqCst);
        let claimed = claim(&harness, &agent, harness.now).await;
        harness
            .worker
            .replay_prepared_for_test(claimed, harness.now)
            .await
            .unwrap();
        assert_eq!(adapter.batches.load(Ordering::SeqCst), 1);
        assert_eq!(
            adapter.individual.load(Ordering::SeqCst),
            0,
            "ambiguous effects must not fall back"
        );
        let pending = harness.stored(&agent.id).await;
        assert_eq!(pending.work.cleanup_debt.len(), 2);
        assert!(pending.work.receipts.is_empty());
        adapter.support.store(retained_support, Ordering::SeqCst);
        let later = harness.now + Duration::from_secs(2);
        let claimed = claim(&harness, &pending, later).await;
        harness
            .worker
            .replay_prepared_for_test(claimed, later)
            .await
            .unwrap();
        assert_eq!(adapter.inspections.load(Ordering::SeqCst), 2);
        assert_eq!(
            adapter.batches.load(Ordering::SeqCst),
            if retained_support { 2 } else { 1 }
        );
        assert_eq!(
            adapter.individual.load(Ordering::SeqCst),
            if retained_support { 0 } else { 2 }
        );
        let completed = harness.stored(&agent.id).await;
        assert_eq!(completed.work.receipts.len(), 2);
        assert!(completed.work.cleanup_debt.is_empty());
    }
}

#[tokio::test]
async fn prepared_batch_refuses_missing_fresh_inspection_without_any_resource_effect() {
    let (mut harness, agent) = prepared().await;
    let adapter = use_adapter(&mut harness, &agent, true);
    adapter.refuse_inspection.store(true, Ordering::SeqCst);
    let claimed = claim(&harness, &agent, harness.now).await;
    assert!(harness
        .worker
        .replay_prepared_for_test(claimed, harness.now)
        .await
        .is_err());
    assert_eq!(adapter.batches.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.individual.load(Ordering::SeqCst), 0);
    assert!(harness.stored(&agent.id).await.work.cleanup_debt.is_empty());
}
