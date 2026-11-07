use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, RuntimeObservation};

struct SlowFailure {
    backend: InMemoryHostedBackend,
    operation: WireOperation,
}

#[async_trait::async_trait]
impl HostedBackend for SlowFailure {
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
        if request.operation() == self.operation {
            tokio::time::sleep(Duration::from_secs(70)).await;
        }
        self.backend.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        if self.operation == WireOperation::OpenAgent {
            tokio::time::sleep(Duration::from_secs(70)).await;
        }
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

#[tokio::test(start_paused = true)]
async fn ledger_hold_delay_starts_after_slow_failure_on_every_effect_path() {
    for (step, operation, _) in CASES {
        let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        h.now = SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
        let initial = prepare(&h, step).await;
        h.adapter = Arc::new(CodexAdapter::new(
            Arc::new(SlowFailure {
                backend: h.backend.clone(),
                operation,
            }),
            CodexSandboxSpec::default(),
        ));
        refuse(&h, operation);
        let at = due(&initial).max(h.now);
        assert!(worker(&h, h.store.clone(), "slow-holder")
            .run_one(&initial.id, at)
            .await
            .unwrap());
        let held = h.stored(&initial.id).await;
        assert_eq!(held.work.step, step);
        assert_eq!(held.status, initial.status);
        assert!(held.work.lease.is_none());
        assert_eq!(
            due(&held),
            at + Duration::from_secs(100),
            "{step:?}: retry must follow the completed failure"
        );
        assert_eq!(held.work.attempts, 1);
    }
}
