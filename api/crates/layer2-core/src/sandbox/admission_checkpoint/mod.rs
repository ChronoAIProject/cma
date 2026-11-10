//! Durable incremental working state for admission and dispatch proofs.
use crate::sandbox::store::SandboxStoreError;
use crate::sandbox::{Agent, AgentLogEntry};
use crate::seconds_from_rfc3339;
use sha2::{Digest, Sha256};
const SEMANTIC_FORMAT: u32 = 1;
use serde::{Deserialize, Serialize};
mod audit;
mod reduce;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MessageRole {
    Owner,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TurnOutcome {
    Completed,
    Failed,
    Interrupted,
    Active,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct QueuedOwnerMessage {
    sequence: u64,
    content_sha256: String,
    nonempty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLogCheckpoint {
    format: u32,
    pub valid: bool,
    pub through_seq: u64,
    pub latest_started_turn_sequence: Option<u64>,
    pub active_turn_sequence: Option<u64>,
    latest_message_sequence: Option<u64>,
    latest_message_role: Option<MessageRole>,
    latest_message_nonempty: bool,
    active_owner_message: bool,
    active_agent_message_count: u64,
    active_gap_frames: u64,
    latest_outcome: Option<TurnOutcome>,
    pub answer_turn_sequence: Option<u64>,
    pub answer_sequence: Option<u64>,
    conversation_id: Option<String>,
    runtime_generation: Option<u64>,
    queued_owner_messages: Vec<QueuedOwnerMessage>,
    steering_expected_sequence: Option<u64>,
    turn_metadata_set: bool,
    #[serde(default)]
    steering_fence_appended_at: Option<String>,
    #[serde(default)]
    creation_seen: bool,
}

impl AgentLogCheckpoint {
    pub fn empty() -> Self {
        Self {
            format: SEMANTIC_FORMAT,
            valid: true,
            through_seq: 0,
            latest_started_turn_sequence: None,
            active_turn_sequence: None,
            latest_message_sequence: None,
            latest_message_role: None,
            latest_message_nonempty: false,
            active_owner_message: false,
            active_agent_message_count: 0,
            active_gap_frames: 0,
            latest_outcome: None,
            answer_turn_sequence: None,
            answer_sequence: None,
            conversation_id: None,
            runtime_generation: None,
            queued_owner_messages: Vec::new(),
            steering_expected_sequence: None,
            turn_metadata_set: false,
            steering_fence_appended_at: None,
            creation_seen: false,
        }
    }

    /// Validate a deserialized checkpoint before trusting persisted state.
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.format != SEMANTIC_FORMAT {
            return Err(SandboxStoreError::Corrupt(
                "unsupported semantic checkpoint format".into(),
            ));
        }
        if self.valid {
            self.validate_internal()?;
        }
        Ok(())
    }

    pub fn from_entries(
        agent: &Agent,
        entries: &[AgentLogEntry],
    ) -> Result<Self, SandboxStoreError> {
        let mut checkpoint = Self::empty();
        if entries.is_empty() {
            checkpoint.queued_owner_messages = queued_messages(agent);
            checkpoint.valid = checkpoint.validate_internal().is_ok()
                && checkpoint.validate_agent_state(agent).is_ok();
            return Ok(checkpoint);
        }
        match &entries[0].kind {
            crate::sandbox::AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } if workspace_id == &agent.workspace_id
                && worktree_id == agent.creation_worktree_id() => {}
            _ => checkpoint.valid = false,
        }

        for entry in entries {
            if checkpoint.through_seq.checked_add(1) != Some(entry.seq) {
                return Err(SandboxStoreError::SequenceConflict);
            }
            if checkpoint.apply_entry(entry).is_err() {
                checkpoint.valid = false;
            }
            if let crate::sandbox::AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                checkpoint.latest_started_turn_sequence = Some(turn_sequence);
            }
            checkpoint.through_seq = entry.seq;
        }
        // The existing full domain fold remains the semantic authority. Some
        // legacy low-level/lifecycle writes expose an unpublished intermediate
        // state. Preserve their structural contract and explicitly disable the
        // checkpoint until a complete valid fold can establish it.
        match crate::sandbox::agent_io::answer_context_for_log(agent, entries) {
            Ok(answer) => {
                checkpoint.answer_turn_sequence = answer.as_ref().map(|value| value.turn_sequence);
                checkpoint.answer_sequence = answer.as_ref().map(|value| value.answer_sequence);
            }
            Err(_) => checkpoint.valid = false,
        }
        checkpoint.queued_owner_messages = queued_messages(agent);
        checkpoint.valid &= checkpoint.validate_internal().is_ok()
            && checkpoint.validate_agent_state(agent).is_ok();
        Ok(checkpoint)
    }

    pub fn latest_turn(&self) -> Option<u64> {
        self.latest_started_turn_sequence
    }

    pub fn answer_context(&self) -> Option<crate::sandbox::agent_io::AnswerContext> {
        self.answer_turn_sequence.zip(self.answer_sequence).map(
            |(turn_sequence, answer_sequence)| crate::sandbox::agent_io::AnswerContext {
                turn_sequence,
                answer_sequence,
            },
        )
    }

    pub fn after_append(
        &self,
        entries: &[AgentLogEntry],
        next_agent: &Agent,
    ) -> Result<Self, SandboxStoreError> {
        let mut next = self.clone();
        for entry in entries {
            if next.through_seq.checked_add(1) != Some(entry.seq) {
                return Err(SandboxStoreError::SequenceConflict);
            }
            if next.through_seq == 0 && !next.creation_seen {
                match &entry.kind {
                    crate::sandbox::AgentLogKind::Created {
                        workspace_id,
                        worktree_id,
                    } if workspace_id == &next_agent.workspace_id
                        && worktree_id == next_agent.creation_worktree_id() => {}
                    _ => next.valid = false,
                }
            }
            if next.apply_entry(entry).is_err() {
                next.valid = false;
            }
            if let crate::sandbox::AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                next.latest_started_turn_sequence = Some(turn_sequence);
            }
            next.through_seq = entry.seq;
        }
        let queued = queued_messages(next_agent);
        next.valid &= next.queued_owner_messages == queued
            && next.validate_internal().is_ok()
            && next.validate_agent_state(next_agent).is_ok();
        next.queued_owner_messages = queued;
        Ok(next)
    }

    fn validate_internal(&self) -> Result<(), SandboxStoreError> {
        let active = self.active_turn_sequence.is_some();
        let outcome_active = matches!(self.latest_outcome, Some(TurnOutcome::Active));
        let has_message = self.latest_message_sequence.is_some();
        let has_role = self.latest_message_role.is_some();
        let has_fence = self.steering_expected_sequence.is_some();
        let has_fence_timestamp = self.steering_fence_appended_at.is_some();
        let invalid_sequence = [
            self.latest_started_turn_sequence,
            self.active_turn_sequence,
            self.latest_message_sequence,
            self.answer_turn_sequence,
            self.answer_sequence,
        ]
        .into_iter()
        .flatten()
        .any(|sequence| sequence == 0 || sequence > self.through_seq);
        let invalid = invalid_sequence
            || self
                .active_turn_sequence
                .is_some_and(|turn| Some(turn) != self.latest_started_turn_sequence)
            || self.answer_context().is_some_and(|answer| {
                answer.validate().is_err()
                    || Some(answer.turn_sequence) != self.latest_started_turn_sequence
                    || !matches!(self.latest_outcome, Some(TurnOutcome::Completed))
            })
            || self.format != SEMANTIC_FORMAT
            || (self.through_seq > 0 && !self.creation_seen)
            || has_message != has_role
            || self.answer_turn_sequence.is_some() != self.answer_sequence.is_some()
            || (self.active_agent_message_count > 0 && !self.active_owner_message)
            || active != outcome_active
            || (!active
                && (self.active_owner_message
                    || self.active_agent_message_count != 0
                    || self.active_gap_frames != 0
                    || self.turn_metadata_set
                    || has_message
                    || has_role
                    || self.latest_message_nonempty
                    || has_fence
                    || has_fence_timestamp))
            || (active && self.answer_context().is_some())
            || has_fence != has_fence_timestamp
            || self
                .steering_fence_appended_at
                .as_deref()
                .is_some_and(|timestamp| seconds_from_rfc3339(timestamp).is_none())
            || self
                .queued_owner_messages
                .iter()
                .enumerate()
                .any(|(index, message)| {
                    message.sequence == 0
                        || self.through_seq < message.sequence
                        || self
                            .queued_owner_messages
                            .iter()
                            .take(index)
                            .any(|previous| previous.sequence == message.sequence)
                });
        if invalid {
            return Err(SandboxStoreError::Corrupt(
                "semantic checkpoint state is internally inconsistent".into(),
            ));
        }
        Ok(())
    }

    pub fn matches_agent(&self, agent: &Agent) -> bool {
        self.validate_agent_state(agent).is_ok()
            && self.queued_owner_messages == queued_messages(agent)
            && (self.through_seq == 0 || self.creation_seen)
    }

    fn validate_agent_state(&self, agent: &Agent) -> Result<(), SandboxStoreError> {
        if self.through_seq != agent.conversation.last_seq
            || self.active_turn_sequence != agent.conversation.active_turn
            || self.conversation_id != agent.conversation.conversation_id
            || self.runtime_generation != agent.conversation.runtime_generation
            || self
                .steering_expected_sequence
                .is_some_and(|expected| expected <= self.through_seq)
        {
            return Err(SandboxStoreError::Corrupt(
                "semantic checkpoint disagrees with Agent conversation state".into(),
            ));
        }
        Ok(())
    }
}

fn queued_messages(agent: &Agent) -> Vec<QueuedOwnerMessage> {
    agent
        .conversation
        .queued_messages
        .iter()
        .filter(|message| message.state == crate::sandbox::QueuedMessageState::Queued)
        .map(|message| QueuedOwnerMessage {
            sequence: message.sequence,
            content_sha256: format!("{:x}", Sha256::digest(message.content.as_bytes())),
            nonempty: !message.content.trim().is_empty(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
