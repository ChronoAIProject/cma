use tokio::sync::mpsc;

use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AgentHomePlan, AgentHomeRequest, AgentLaunch, AgentOpenRequest, ConversationChannel,
    ConversationUpdate, Result, RuntimeHandle, RuntimeObservation,
};

/// Kernel-specific settings used while opening its ACP conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedAgentOpenSettings {
    pub permission_mode: Option<String>,
    pub own_credential: bool,
}

/// Provider-specific rendering needed by the otherwise shared hosted adapter.
pub trait HostedAgentRuntime: Send + Sync {
    fn home_plan(&self, request: &AgentHomeRequest) -> Result<AgentHomePlan>;
    fn launch(&self, request: &AgentOpenRequest) -> Result<AgentLaunch>;

    fn open_settings(&self, request: &AgentOpenRequest) -> Result<HostedAgentOpenSettings> {
        Ok(HostedAgentOpenSettings {
            permission_mode: None,
            own_credential: request.options.own_credential,
        })
    }
}

/// Provider lifecycle and supervisor transport used by a hosted adapter.
#[async_trait::async_trait]
pub trait HostedBackend: Send + Sync {
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle>;

    async fn inventory(&self) -> Result<Vec<RuntimeObservation>> {
        Err(crate::AdapterError::Unsupported(
            "hosted runtime inventory is not supported".into(),
        ))
    }

    async fn adopt(
        &self,
        _handle: &RuntimeHandle,
        _request: &ProviderSandboxRequest,
    ) -> Result<Option<RuntimeObservation>> {
        Err(crate::AdapterError::Unsupported(
            "hosted runtime adoption is not supported".into(),
        ))
    }

    async fn reclaim(&self, _observation: &RuntimeObservation) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "hosted runtime reclamation is not supported".into(),
        ))
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame>;

    /// Open keeps the ACP channel out of the JSON control receipt while using
    /// the same launch frame that Layer 4 acknowledges.
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)>;

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()>;
}
