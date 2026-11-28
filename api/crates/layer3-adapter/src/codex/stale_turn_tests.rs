use super::*;
use crate::{
    provider::ProviderSandboxRequest,
    stale_turn::{StaleTurnEvidence, StaleTurnHold, StaleTurnProducer, StaleTurnTarget},
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    Result,
};

struct QualifiedBackend {
    runtime: RuntimeHandle,
}

#[async_trait::async_trait]
impl StaleTurnProducer for QualifiedBackend {
    async fn observe_stale_turn_seal(
        &self,
        _: &StaleTurnTarget,
        _: usize,
    ) -> std::result::Result<StaleTurnEvidence, StaleTurnHold> {
        panic!("capability selection must perform no native I/O")
    }
}

#[async_trait::async_trait]
impl HostedBackend for QualifiedBackend {
    fn stale_turn_producer(&self, runtime: &RuntimeHandle) -> Option<&dyn StaleTurnProducer> {
        (&self.runtime == runtime).then_some(self)
    }

    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        panic!("capability selection must not provision a runtime")
    }

    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("capability selection must not exchange control frames")
    }

    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("capability selection must not open an Agent")
    }

    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("capability selection must not tear down a runtime")
    }
}

#[test]
fn codex_exposes_the_backend_stale_turn_producer_for_only_the_qualified_runtime() {
    let qualified = RuntimeHandle("agent-sandbox:fixture-member:fixture-uid".into());
    let backend = Arc::new(QualifiedBackend {
        runtime: qualified.clone(),
    });
    let adapter = CodexAdapter::new(backend.clone(), CodexSandboxSpec::default());
    let adapter: &dyn SandboxAdapter = &adapter;
    let producer = adapter.stale_turn_producer(&qualified).unwrap();
    assert!(std::ptr::addr_eq(
        producer,
        backend.as_ref() as &dyn StaleTurnProducer
    ));
    let other_runtime = RuntimeHandle("opensandbox:fixture-predecessor".into());
    assert!(adapter.stale_turn_producer(&other_runtime).is_none());
}
