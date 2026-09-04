use tokio::sync::mpsc;

use crate::provider::ProviderSandboxRequest;
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{
    AgentHomePlan, AgentHomeRequest, AgentLaunch, AgentOpenRequest, ConversationChannel,
    ConversationUpdate, Result, RuntimeHandle,
};

/// Provider-specific rendering needed by the otherwise shared hosted adapter.
pub trait HostedAgentRuntime: Send + Sync {
    fn home_plan(&self, request: &AgentHomeRequest) -> Result<AgentHomePlan>;
    fn launch(&self, request: &AgentOpenRequest) -> Result<AgentLaunch>;
}

/// Provider lifecycle and supervisor transport used by a hosted adapter.
#[async_trait::async_trait]
pub trait HostedBackend: Send + Sync {
    async fn provision(&self, request: ProviderSandboxRequest) -> Result<RuntimeHandle>;

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
        launch: AgentLaunch,
        resume_conversation: Option<String>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel>;

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()>;
}
