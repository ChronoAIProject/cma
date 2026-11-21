//! The production outer adapter must preserve optional provider diagnostics.
use super::{CodexAdapter, CodexSandboxSpec};
use crate::hosted::{HostedAgentOpenSettings, HostedBackend};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle,
    RuntimeTermination, SandboxAdapter,
};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Default)]
struct Diagnostics(Mutex<Vec<RuntimeHandle>>);

#[async_trait::async_trait]
impl HostedBackend for Diagnostics {
    async fn runtime_termination(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<Option<RuntimeTermination>> {
        self.0.lock().unwrap().push(handle.clone());
        match handle.as_str() {
            "agent-sandbox:owned:uid" => Ok(Some(RuntimeTermination {
                out_of_memory: true,
                finished_at: "2026-10-16T09:00:00Z".into(),
            })),
            "agent-sandbox:unknown:uid" => Ok(None),
            _ => Err(AdapterError::Unavailable("diagnostics unavailable".into())),
        }
    }

    async fn provision(&self, _: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        panic!("diagnostics must not provision")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("diagnostics must not issue runtime commands")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("diagnostics must not open an Agent")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("diagnostics must not delete a runtime")
    }
}

#[tokio::test]
async fn outer_adapter_forwards_exact_termination_evidence_absence_and_errors_without_mutation() {
    let backend = Arc::new(Diagnostics::default());
    let adapter: Arc<dyn SandboxAdapter> = Arc::new(CodexAdapter::new(
        backend.clone(),
        CodexSandboxSpec::default(),
    ));
    let owned = RuntimeHandle::new("agent-sandbox:owned:uid");
    let unknown = RuntimeHandle::new("agent-sandbox:unknown:uid");
    let unavailable = RuntimeHandle::new("agent-sandbox:unavailable:uid");
    assert_eq!(
        adapter.runtime_termination(&owned).await.unwrap(),
        Some(RuntimeTermination {
            out_of_memory: true,
            finished_at: "2026-10-16T09:00:00Z".into(),
        })
    );
    assert_eq!(adapter.runtime_termination(&unknown).await.unwrap(), None);
    assert!(matches!(
        adapter.runtime_termination(&unavailable).await,
        Err(AdapterError::Unavailable(_))
    ));
    assert_eq!(
        *backend.0.lock().unwrap(),
        vec![owned, unknown, unavailable]
    );
}
