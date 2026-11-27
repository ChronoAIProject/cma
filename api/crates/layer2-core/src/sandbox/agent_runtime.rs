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

/// Retain the authority of the exact operation that requested recovery, even
/// when cancellation drops the operation before its delivery result settles.
pub(super) struct ReconnectOnDrop {
    runtimes: std::sync::Arc<dyn AgentRuntimeRegistry>,
    target: AgentRuntimeTarget,
    owner: Option<std::sync::Arc<crate::agent_ownership::Authority>>,
    pub(super) armed: bool,
}

impl ReconnectOnDrop {
    pub(super) fn new(
        runtimes: std::sync::Arc<dyn AgentRuntimeRegistry>,
        target: AgentRuntimeTarget,
    ) -> Self {
        Self {
            runtimes,
            target,
            owner: crate::agent_ownership::execution_authority(),
            armed: true,
        }
    }
}

impl Drop for ReconnectOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let runtimes = self.runtimes.clone();
            let target = self.target.clone();
            let owner = self.owner.clone();
            tokio::spawn(async move {
                let recovery = runtimes.request_reconnect(&target);
                if let Some(owner) = owner {
                    let _ = owner.run(recovery).await;
                } else {
                    let _ = recovery.await;
                }
            });
        }
    }
}
