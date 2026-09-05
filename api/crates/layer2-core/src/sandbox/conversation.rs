use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::AgentLogEntry;

pub const MAX_DURABLE_AGENT_MESSAGE_BYTES: usize = 1024 * 1024;

/// Live message fragments for one runtime generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentMessageAssembly {
    pub(crate) message_id: Option<String>,
    pub(crate) content: String,
    pub(crate) prompt_evidence: bool,
}

impl AgentMessageAssembly {
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentUpdateOutcome {
    pub appended: Vec<AgentLogEntry>,
    pub turn_closed: bool,
}

impl AgentUpdateOutcome {
    pub(crate) fn quiet() -> Self {
        Self {
            appended: Vec::new(),
            turn_closed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentConversationError {
    #[error("agent not found")]
    NotFound,
    #[error("agent runtime generation is stale")]
    StaleGeneration,
    #[error("agent is not in a valid conversation state: {0}")]
    InvalidState(&'static str),
    #[error("agent conversation data is not usable: {0}")]
    Corrupt(String),
    #[error("agent conversation store is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait AgentUpdateRecorder: Send + Sync {
    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError>;
}
