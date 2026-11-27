pub use layer3_adapter::{
    AdapterError as AgentRuntimeInputError, ConversationInput as AgentRuntimeInput,
    ConversationInputSender as AgentRuntimeInputSender, ConversationUpdate as AgentRuntimeUpdate,
    PromptResponse as AgentRuntimePromptResponse, TurnStopReason as AgentRuntimeTurnStopReason,
};

use crate::{AgentId, RuntimeCapabilities};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeTarget {
    pub agent_id: AgentId,
    pub conversation_id: String,
    pub runtime_generation: u64,
}

#[derive(Debug, Clone)]
pub struct AgentRuntimeConnection {
    pub capabilities: RuntimeCapabilities,
    pub input: AgentRuntimeInputSender,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentRuntimeError {
    #[error("agent runtime connection not found")]
    NotFound,
    #[error("agent runtime generation is stale")]
    StaleGeneration,
    #[error("agent runtime registry is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait AgentRuntimeRegistry: Send + Sync {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError>;

    /// Request recovery before a claimed command binds any runtime input.
    /// Implementations that own runtime attachments can schedule durable work.
    async fn recover_missing_attachment(&self, _target: &AgentRuntimeTarget) {}

    /// Retire a failed input attachment and schedule durable reattachment.
    async fn request_reconnect(
        &self,
        _target: &AgentRuntimeTarget,
    ) -> Result<(), AgentRuntimeError> {
        Err(AgentRuntimeError::Unavailable(
            "runtime reconnection is not configured".into(),
        ))
    }

    /// Idempotently retire every interaction route at or below this authority.
    async fn fence(
        &self,
        agent_id: &AgentId,
        authority_fence: u64,
    ) -> Result<(), AgentRuntimeError>;
}
