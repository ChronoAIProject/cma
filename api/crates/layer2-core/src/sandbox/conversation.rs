use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::AgentLogEntry;

pub const MAX_DURABLE_AGENT_MESSAGE_BYTES: usize = 1024 * 1024;

/// Live message fragments for one runtime generation.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageAssembly {
    pub(crate) message_id: Option<String>,
    pub(crate) content: String,
    pub(crate) prompt_evidence: bool,
    pub(crate) live_message: Option<super::conversation_recorder::live::AgentLiveMessageIdentity>,
    pub(crate) live_thought: Option<super::conversation_recorder::live::AgentLiveMessageIdentity>,
    #[serde(skip)]
    pub(crate) pending_update: Option<Box<PendingAgentUpdate>>,
    /// A native-history proof being applied.  It is process-local and never
    /// persisted in the checkpoint; each CAS retry revalidates it.
    #[serde(skip)]
    pub(crate) reconciliation: Option<NativeCompletionEvidence>,
}

/// Runtime evidence required before a missing terminal can be synthesized.
/// The API obtains this from the bounded native history export and rechecks it
/// while holding the Agent writer CAS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCompletionEvidence {
    pub native_conversation_id: String,
    pub runtime_generation: u64,
    pub turn_sequence: u64,
    pub native_turn_id: String,
    pub admitted_at: String,
    pub completed_at: String,
    pub task_started: bool,
    pub task_complete: bool,
    pub no_later_open_turn: bool,
    pub history_complete: bool,
    pub history_prefix_unchanged: bool,
    pub history_sha256: String,
}

impl NativeCompletionEvidence {
    pub fn validate_against(&self, agent: &super::Agent) -> Result<(), AgentConversationError> {
        let invalid = || {
            AgentConversationError::InvalidState("native completion proof no longer matches Agent")
        };
        let admitted = crate::seconds_from_rfc3339(&self.admitted_at);
        let completed = crate::seconds_from_rfc3339(&self.completed_at);
        if self.runtime_generation == 0
            || self.turn_sequence == 0
            || self.native_conversation_id.is_empty()
            || self.native_turn_id.is_empty()
            || self.history_sha256.len() != 64
            || !self
                .history_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !self.task_started
            || !self.task_complete
            || !self.no_later_open_turn
            || !self.history_complete
            || !self.history_prefix_unchanged
            || admitted.is_none()
            || completed.is_none()
            || completed < admitted
            || !matches!(
                agent.status,
                super::AgentStatus::Working | super::AgentStatus::WaitingApproval
            )
            || agent.conversation.conversation_id.as_deref()
                != Some(self.native_conversation_id.as_str())
            || agent.conversation.runtime_generation != Some(self.runtime_generation)
            || agent.conversation.active_turn != Some(self.turn_sequence)
            || !agent
                .conversation
                .observations
                .usage
                .as_ref()
                .is_some_and(|usage| {
                    usage.source_turn == self.native_turn_id
                        && usage.current_turn.map(|turn| turn.turn_sequence)
                            == Some(self.turn_sequence)
                })
        {
            return Err(invalid());
        }
        let Some(operation) = agent.conversation.operation.as_ref() else {
            return Err(invalid());
        };
        if operation.runtime_generation != self.runtime_generation
            || operation.turn_sequence != self.turn_sequence
            || !matches!(
                operation.intent,
                super::AgentOperationIntent::SendMessage { .. }
            )
            || matches!(
                operation.state,
                super::AgentOperationDelivery::PreWriteFailed
                    | super::AgentOperationDelivery::RuntimeLost
            )
            || agent
                .conversation
                .queued_messages
                .iter()
                .any(|message| message.sequence > operation.sequence)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingAgentUpdate {
    pub id: String,
    pub fingerprint: String,
    pub runtime_generation: u64,
    pub active_turn: Option<u64>,
    pub after_seq: u64,
}

impl AgentMessageAssembly {
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    #[error("agent conversation log capacity exceeded")]
    CapacityExceeded,
    #[error("agent conversation store is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait AgentUpdateRecorder: Send + Sync {
    async fn record_reopen_update(
        &self,
        _id: &AgentId,
        _guard: &super::store::AgentReopenGuard,
        _assembly: &mut AgentMessageAssembly,
        _update: ConversationUpdate,
        _appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        Err(AgentConversationError::InvalidState(
            "durable reopen recording is unsupported",
        ))
    }

    fn subscribe_live(&self, _id: &AgentId) -> Option<super::AgentLiveSubscription> {
        None
    }

    async fn restore_assembly(
        &self,
        _id: &AgentId,
        _runtime_generation: u64,
        _assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        Ok(())
    }

    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError>;

    /// Apply a native completion proof through the same durable fold used by
    /// live output. Implementations must fail closed when the proof or any
    /// current control-state fence changes.
    async fn reconcile_completion(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        evidence: NativeCompletionEvidence,
        response: layer3_adapter::PromptResponse,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        let _ = (
            id,
            runtime_generation,
            assembly,
            evidence,
            response,
            appended_at,
        );
        Err(AgentConversationError::InvalidState(
            "native completion reconciliation is unsupported",
        ))
    }
}
