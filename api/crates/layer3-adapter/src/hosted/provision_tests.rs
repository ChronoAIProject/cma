use tokio::sync::mpsc;

use super::HostedBackend;
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle,
    SandboxAdapter, SandboxSpec,
};

struct ReusedHandleBackend;

#[async_trait::async_trait]
impl HostedBackend for ReusedHandleBackend {
    async fn provision(&self, _request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        Ok(RuntimeHandle("provider-reused-handle".into()))
    }

    async fn exchange(
        &self,
        _handle: &RuntimeHandle,
        _request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        Err(AdapterError::Unavailable("not used by this test".into()))
    }

    async fn open_agent(
        &self,
        _handle: &RuntimeHandle,
        _launch: AgentLaunch,
        _resume_conversation: Option<String>,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, AgentOpenReceipt)> {
        Err(AdapterError::Unavailable("not used by this test".into()))
    }

    async fn teardown(&self, _handle: &RuntimeHandle) -> crate::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn one_provider_handle_cannot_attach_to_two_stable_identities() {
    let adapter = CodexAdapter::with_backend(ReusedHandleBackend, CodexSandboxSpec::default());
    let first = SandboxSpec {
        sandbox_id: "sandbox-a".into(),
        ..SandboxSpec::default()
    };
    assert_eq!(
        adapter.provision(&first).await.unwrap().as_str(),
        "provider-reused-handle"
    );

    let changed = SandboxSpec {
        sandbox_id: "sandbox-b".into(),
        ..first
    };
    let error = adapter.provision(&changed).await.unwrap_err();
    assert!(matches!(error, AdapterError::Rejected(detail)
        if detail.contains("reused live runtime handle")));
}
