use crate::sandbox::{
    Agent, AgentMessageAssembly, AgentUpdateOutcome, MAX_DURABLE_AGENT_MESSAGE_BYTES,
};

use super::{SandboxStoreError, MAX_AGENT_LOG_BATCH_SIZE};

/// Private persistence sidecar. This never changes the public Agent record or log schema.
/// The last bounded update receipt permits exact recovery after an uncertain store reply.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageCheckpoint {
    pub version: u32,
    pub update_id: String,
    pub update_fingerprint: String,
    pub native_conversation_id: String,
    pub runtime_generation: u64,
    pub active_turn: Option<u64>,
    pub after_seq: u64,
    pub assembly: AgentMessageAssembly,
    pub outcome: AgentUpdateOutcome,
    /// Survives turn boundaries; scoped to this native conversation and generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<layer3_adapter::RuntimeOutputSource>,
}

impl AgentMessageCheckpoint {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        let invalid = || SandboxStoreError::Corrupt("invalid agent message checkpoint".into());
        if self.version != 1
            || uuid::Uuid::parse_str(&self.update_id).is_err()
            || self.update_fingerprint.len() != 64
            || !self
                .update_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.native_conversation_id.is_empty()
            || self.native_conversation_id.len() > 4096
            || self.runtime_generation == 0
            || self.active_turn == Some(0)
            || (self.active_turn.is_none() && !self.assembly.is_empty())
            || self.assembly.content.len() > MAX_DURABLE_AGENT_MESSAGE_BYTES
            || (self.assembly.text_published && self.assembly.is_empty())
            || self
                .assembly
                .message_id
                .as_ref()
                .is_some_and(|id| id.len() > 4096)
            || self.assembly.pending_update.is_some()
            || self.assembly.output_state.is_some()
            || self.outcome.appended.len() > MAX_AGENT_LOG_BATCH_SIZE
            || self
                .outcome
                .appended
                .iter()
                .any(|entry| entry.seq == 0 || entry.seq > self.after_seq)
            || self
                .outcome
                .appended
                .windows(2)
                .any(|entries| entries[0].seq.checked_add(1) != Some(entries[1].seq))
        {
            return Err(invalid());
        }
        if let Some(output) = &self.output {
            if output.native_conversation_id != self.native_conversation_id
                || uuid::Uuid::parse_str(&output.stream_id).is_err()
                || output.cursor == 0
                || output.payload_sha256.len() != 64
                || !output
                    .payload_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(invalid());
            }
        }
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() > 8 * 1024 * 1024 {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn matches(&self, agent: &Agent) -> bool {
        agent.conversation.conversation_id.as_deref() == Some(&self.native_conversation_id)
            && agent.conversation.runtime_generation == Some(self.runtime_generation)
            && agent.conversation.active_turn == self.active_turn
            && agent.conversation.last_seq == self.after_seq
    }

    /// Candidate anchor-only repair after the recorder verifies the intervening log.
    /// Preserve the exact update receipt and prefix; never cross a turn or identity.
    pub fn reanchored(&self, agent: &Agent) -> Option<Self> {
        if self.assembly.is_empty() || self.after_seq >= agent.conversation.last_seq {
            return None;
        }
        let mut next = self.clone();
        next.after_seq = agent.conversation.last_seq;
        next.matches(agent).then_some(next)
    }

    /// Other semantic writes may append control evidence while the same message is active.
    /// Carry its anchor in that transaction, but never carry a prefix into a different turn.
    pub fn advance(&self, current: &Agent, next: &Agent) -> Option<Self> {
        // A lifecycle recorder may already own replacement-generation output.
        // Publication carries that exact durable cursor into ordinary recording.
        if current.conversation.runtime_generation != next.conversation.runtime_generation
            && next.conversation.runtime_generation == Some(self.runtime_generation)
            && current.conversation.conversation_id == next.conversation.conversation_id
            && next.conversation.conversation_id.as_deref() == Some(&self.native_conversation_id)
            && current.conversation.active_turn.is_none()
            && next.conversation.active_turn.is_none()
            && self.active_turn.is_none()
            && self.assembly.is_empty()
        {
            let mut checkpoint = self.clone();
            checkpoint.after_seq = next.conversation.last_seq;
            return Some(checkpoint);
        }
        if current.conversation.conversation_id != next.conversation.conversation_id
            || current.conversation.runtime_generation != next.conversation.runtime_generation
            || current.conversation.active_turn != next.conversation.active_turn
        {
            let mut checkpoint = self.clone();
            checkpoint.assembly = AgentMessageAssembly::default();
            if current.conversation.conversation_id != next.conversation.conversation_id
                || current.conversation.runtime_generation != next.conversation.runtime_generation
            {
                checkpoint.output = None;
            }
            return Some(checkpoint);
        }
        if !self.matches(current) {
            return Some(self.clone());
        }
        let mut checkpoint = self.clone();
        checkpoint.after_seq = next.conversation.last_seq;
        Some(checkpoint)
    }
}

/// Agent and private checkpoint from one storage snapshot. Callers must still
/// use the commit CAS: semantic history can advance after this read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessageSnapshot {
    pub agent: Agent,
    pub checkpoint: Option<AgentMessageCheckpoint>,
}
