//! Incremental semantic facts. Text remains in the original immutable events;
//! neither a long turn nor a long history grows this current-state record.
use serde::{Deserialize, Serialize};

use super::{
    agent_reads_usage::UsageFold, Agent, AgentLogEntry, AgentLogKind, AgentOperationDelivery,
    AgentOperationKind, AgentTranscriptRole, AgentTurnOutcome, AgentTurnUsage, AgentUsage,
};

mod legacy;
mod observe;
pub use legacy::BoundedAgentLogFold;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLogProjectionState {
    pub through_seq: u64,
    /// Max event.runtime_generation, independently for each native boundary kind.
    /// Zero is a certified absence only when this state covers the captured head.
    pub max_opened_generation: u64,
    pub max_retired_generation: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_conversation: Option<(String, u64)>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_turn: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_task_status: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub title_turn: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_commands: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_plan: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_metadata: Option<u64>,
    pub has_owner_input: bool,
    pub approval_epoch: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub active: Option<AgentLogTurnFacts>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_completed: Option<AgentLogTurnFacts>,
    #[serde(deserialize_with = "required_nullable")]
    pub open_completion: Option<(u64, String)>,
    pub duplicate_open_completion: bool,
    #[serde(deserialize_with = "required_nullable")]
    pub steering: Option<(u64, String)>,
    pub unconsumed_queue: Vec<AgentLogQueuedReference>,
    usage: UsageFold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLogQueuedReference {
    pub sequence: u64,
    pub payload_sha256: String,
}

/// Original immutable references from one certified source snapshot. A store
/// reads all three together so an active segment is validated only once.
pub struct AgentLogAnswerEvidence {
    pub facts: AgentLogTurnFacts,
    pub start: AgentLogEntry,
    pub message: AgentLogEntry,
    pub terminal: AgentLogEntry,
}

pub enum AgentLogTurnLookup {
    Legacy,
    Inactive,
    Active {
        start: Box<AgentLogEntry>,
        gap: Option<Box<AgentLogEntry>>,
    },
}

pub enum AgentLogAnswerLookup {
    Legacy,
    NotFinal,
    Answer(Box<AgentLogAnswerEvidence>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLogTurnFacts {
    pub turn_sequence: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub previous_turn: Option<u64>,
    pub started_at: String,
    #[serde(deserialize_with = "required_nullable")]
    pub origin_key: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub model: Option<String>,
    pub conversation_id: String,
    pub runtime_generation: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub usage: Option<AgentTurnUsage>,
    #[serde(deserialize_with = "required_nullable")]
    pub usage_sequence: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub owner_sequence: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub first_assistant_sequence: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub last_message: Option<(u64, AgentTranscriptRole, bool)>,
    #[serde(deserialize_with = "required_nullable")]
    pub metadata_sequence: Option<u64>,
    pub lost_frames: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub first_gap_sequence: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub end_sequence: Option<u64>,
    pub source_completed: bool,
    pub title_eligible: bool,
    pub outcome: AgentTurnOutcome,
}

impl AgentLogTurnFacts {
    pub fn validate(&self, through_seq: u64) -> Result<(), String> {
        let covered = |sequence| sequence > 0 && sequence <= through_seq;
        if !covered(self.turn_sequence)
            || self.conversation_id.trim().is_empty()
            || self.conversation_id.len() > 512
            || self.runtime_generation == 0
            || self.model.as_ref().is_some_and(|model| {
                model.len() > 256 || model.trim() != model || model.chars().any(char::is_control)
            })
            || self.usage.is_some() != self.usage_sequence.is_some()
            || self
                .usage
                .is_some_and(|usage| usage.turn_sequence != self.turn_sequence)
            || self
                .usage_sequence
                .is_some_and(|seq| !covered(seq) || seq <= self.turn_sequence)
            || self
                .previous_turn
                .is_some_and(|seq| seq == 0 || seq >= self.turn_sequence)
            || crate::seconds_from_rfc3339(&self.started_at).is_none()
            || self
                .owner_sequence
                .is_none_or(|seq| !covered(seq) || seq == self.turn_sequence)
            || self
                .first_assistant_sequence
                .is_some_and(|seq| !covered(seq) || seq <= self.turn_sequence)
            || self.last_message.as_ref().is_none_or(|(seq, role, _)| {
                !covered(*seq)
                    || (*role == AgentTranscriptRole::Agent && *seq <= self.turn_sequence)
            })
            || self
                .metadata_sequence
                .is_some_and(|seq| !covered(seq) || seq <= self.turn_sequence)
            || self.end_sequence.is_some_and(|seq| {
                !covered(seq)
                    || seq <= self.turn_sequence
                    || self
                        .last_message
                        .as_ref()
                        .is_some_and(|(last, _, _)| *last >= seq)
            })
            || (self.first_gap_sequence.is_some() != (self.lost_frames > 0))
            || self.first_gap_sequence.is_some_and(|seq| {
                !covered(seq)
                    || seq <= self.turn_sequence
                    || self.end_sequence.is_some_and(|end| seq >= end)
            })
            || (self.end_sequence.is_none() != matches!(self.outcome, AgentTurnOutcome::Active))
            || (self.source_completed && self.end_sequence.is_none())
            || (!self.source_completed
                && matches!(self.outcome, AgentTurnOutcome::Completed { .. }))
            || (self.lost_frames > 0 && matches!(self.outcome, AgentTurnOutcome::Completed { .. }))
        {
            return Err("Agent turn evidence is outside its certified source interval".into());
        }
        Ok(())
    }

    pub fn final_answer_sequence(&self) -> Option<u64> {
        if !matches!(self.outcome, AgentTurnOutcome::Completed { .. }) || self.lost_frames != 0 {
            return None;
        }
        self.last_message
            .as_ref()
            .filter(|(_, role, nonempty)| *role == AgentTranscriptRole::Agent && *nonempty)
            .map(|(seq, _, _)| *seq)
    }
}

/// At most a fixed number of evidence upserts per source event. Sets that grow
/// with history live in indexed rows, never arrays in the current-state record.
pub enum AgentLogEvidenceChange {
    Turn(Box<AgentLogTurnFacts>),
    Message {
        sequence: u64,
        turn_sequence: u64,
        role: AgentTranscriptRole,
    },
    Queued {
        sequence: u64,
        consumed_by: Option<u64>,
    },
    Approval {
        approval_id: String,
        request_sequence: Option<u64>,
        turn_sequence: Option<u64>,
        epoch: u64,
    },
    Operation {
        operation_sequence: u64,
        sequence: u64,
    },
}

impl AgentLogProjectionState {
    pub fn usage(&self) -> AgentUsage {
        self.usage.reading()
    }

    /// A queued owner reference must be loaded from the exact indexed evidence
    /// and original source event before passing it here.
    pub fn queued_owner(entry: &AgentLogEntry) -> Option<u64> {
        match &entry.kind {
            AgentLogKind::TurnStarted {
                origin_key: Some(key),
                ..
            } => super::queued_message::queued_message_origin_sequence(key),
            _ => None,
        }
    }

    pub fn observe(
        &mut self,
        agent: &Agent,
        entry: &AgentLogEntry,
        queued_owner: Option<u64>,
    ) -> Result<Vec<AgentLogEvidenceChange>, String> {
        if entry.agent_id != agent.id
            || self.through_seq.checked_add(1) != Some(entry.seq)
            || entry.seq > 9_007_199_254_740_991
            || crate::seconds_from_rfc3339(&entry.appended_at).is_none()
            || queued_owner != Self::queued_owner(entry)
        {
            return Err("Agent log projection source identity is invalid".into());
        }
        if self.steering.as_ref().is_some_and(|(seq, at)| {
            seq.checked_add(1) != Some(entry.seq)
                || at != &entry.appended_at
                || !matches!(entry.kind, AgentLogKind::OwnerMessage { .. })
        }) {
            return Err("Agent steering fence has no adjacent owner message".into());
        }
        let changes = self.apply(agent, entry, queued_owner)?;
        match entry.kind {
            AgentLogKind::ConversationOpened {
                runtime_generation, ..
            } => {
                self.max_opened_generation = self.max_opened_generation.max(runtime_generation);
            }
            AgentLogKind::RuntimeGenerationRetired {
                runtime_generation, ..
            } => {
                self.max_retired_generation = self.max_retired_generation.max(runtime_generation);
            }
            _ => {}
        }
        self.through_seq = entry.seq;
        Ok(changes)
    }

    pub fn validate_aggregate(&self, agent: &Agent) -> Result<(), String> {
        agent
            .validate_worktree_recoveries()
            .map_err(|e| e.to_string())?;
        let maximum = self.max_opened_generation.max(self.max_retired_generation);
        if self
            .latest_conversation
            .as_ref()
            .map(|(_, generation)| *generation)
            != (maximum != 0).then_some(maximum)
            || (maximum != 0 && self.max_opened_generation == 0)
            || (self.max_retired_generation != 0
                && self.max_retired_generation == self.max_opened_generation)
            || maximum > 9_007_199_254_740_991
        {
            return Err("Agent generation maxima disagree with the certified projection".into());
        }
        if self.through_seq != agent.conversation.last_seq
            || self.steering.is_some()
            || self.active.as_ref().map(|turn| turn.turn_sequence) != agent.conversation.active_turn
            || (self.active.is_none()
                && self
                    .latest_completed
                    .as_ref()
                    .map(|turn| turn.turn_sequence)
                    != self.latest_turn)
            || self
                .latest_turn
                .is_some_and(|seq| seq == 0 || seq > self.through_seq)
            || self
                .active
                .as_ref()
                .is_some_and(|turn| Some(turn.turn_sequence) != self.latest_turn)
            || self
                .title_turn
                .is_some_and(|seq| seq == 0 || self.latest_turn.is_none_or(|last| seq > last))
            || (self.through_seq > 0
                && self
                    .latest_task_status
                    .is_none_or(|seq| seq == 0 || seq > self.through_seq))
            || (self.through_seq == 0 && self.latest_task_status.is_some())
            || self
                .latest_turn
                .is_some_and(|turn| self.latest_task_status.is_none_or(|seq| seq < turn))
            || self.unconsumed_queue.len() > super::MAX_QUEUED_AGENT_MESSAGES
            || [self.latest_commands, self.latest_plan, self.latest_metadata]
                .into_iter()
                .flatten()
                .any(|seq| seq == 0 || seq > self.through_seq)
            || self.latest_plan.is_some_and(|seq| {
                self.active
                    .as_ref()
                    .is_none_or(|turn| seq <= turn.turn_sequence)
            })
            || self.unconsumed_queue.iter().any(|source| {
                !agent.conversation.queued_messages.iter().any(|queued| {
                    queued.state == super::QueuedMessageState::Queued
                        && queued.sequence == source.sequence
                        && queued.payload_sha256 == source.payload_sha256
                })
            })
        {
            return Err("Agent log projection does not cover the aggregate".into());
        }
        if let Some(turn) = &self.active {
            turn.validate(self.through_seq)?;
        }
        if let Some(turn) = &self.latest_completed {
            turn.validate(self.through_seq)?;
            if turn.end_sequence.is_none()
                || self.latest_turn.is_none_or(|seq| turn.turn_sequence > seq)
                || self
                    .active
                    .as_ref()
                    .is_some_and(|active| turn.turn_sequence >= active.turn_sequence)
            {
                return Err("Agent completed turn projection is inconsistent".into());
            }
        }
        let native = agent
            .conversation
            .conversation_id
            .as_ref()
            .zip(agent.conversation.runtime_generation);
        match (native, &self.latest_conversation) {
            (None, None) => Ok(()),
            (Some((id, generation)), Some((logged, logged_generation)))
                if id == logged && generation == *logged_generation =>
            {
                Ok(())
            }
            (Some((id, 1)), None) if !id.trim().is_empty() => {
                if self.duplicate_open_completion {
                    return Err("Agent initial native open is duplicated".into());
                }
                let entries = self
                    .open_completion
                    .iter()
                    .map(|(seq, at)| AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: *seq,
                        appended_at: at.clone(),
                        kind: AgentLogKind::WorkStepCompleted {
                            step: super::AgentWorkStep::OpenConversation,
                        },
                    })
                    .collect::<Vec<_>>();
                // Reuse the existing unpublished-native identity proof.
                super::agent_reads_fold::validate_projection_identity(agent, &entries)
            }
            _ => Err("Agent conversation identity disagrees with its projection".into()),
        }
    }
}

// Nullable durable fields are required; omission must not silently become None.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
mod tests;
