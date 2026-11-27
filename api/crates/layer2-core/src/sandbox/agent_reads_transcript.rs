use super::agent_reads_fold::{transcript_message, TranscriptFold};
use super::{
    Agent, AgentLogEntry, AgentLogKind, AgentOperationDelivery, AgentOperationKind,
    AgentTranscript, AgentTurnOutcome,
};
use crate::seconds_from_rfc3339;

/// Incremental form of the exact transcript fold. Durable stores use this
/// when a trusted reader visits selected source rows page by page; the fold
/// retains only the public transcript being built and never a second copy of
/// the complete Agent log.
#[derive(Default)]
pub struct AgentTranscriptFold {
    transcript: TranscriptFold,
    latest_conversation: Option<(String, u64)>,
    previous_seq: u64,
    pending_steering: Option<(u64, String)>,
    saw_creation: bool,
}

impl AgentTranscriptFold {
    pub fn push(&mut self, agent: &Agent, entry: AgentLogEntry) -> Result<(), String> {
        if entry.agent_id != agent.id
            || entry.seq == 0
            || entry.seq <= self.previous_seq
            || seconds_from_rfc3339(&entry.appended_at).is_none()
        {
            return Err("agent transcript row identity or sequence is invalid".into());
        }
        self.previous_seq = entry.seq;
        match &entry.kind {
            AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } if entry.seq == 1
                && workspace_id == &agent.workspace_id
                && worktree_id == agent.creation_worktree_id() =>
            {
                if self.saw_creation {
                    return Err("agent log contains duplicate creation".into());
                }
                self.saw_creation = true;
            }
            AgentLogKind::Created { .. } => {
                return Err("agent creation log lineage does not match".into());
            }
            AgentLogKind::ConversationOpened {
                conversation_id,
                runtime_generation,
            } => {
                if self.transcript.active.is_some()
                    || conversation_id.trim().is_empty()
                    || conversation_id.len() > 512
                    || *runtime_generation == 0
                    || self
                        .latest_conversation
                        .as_ref()
                        .is_some_and(|(_, previous)| runtime_generation <= previous)
                {
                    return Err("agent conversation boundary is invalid".into());
                }
                self.latest_conversation = Some((conversation_id.clone(), *runtime_generation));
            }
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            } => {
                let Some((_, generation)) = self.latest_conversation.as_mut() else {
                    return Err("agent runtime retirement has no conversation".into());
                };
                if self.transcript.active.is_some()
                    || *generation != *previous_generation
                    || previous_generation.checked_add(1) != Some(*runtime_generation)
                {
                    return Err("agent runtime retirement boundary is invalid".into());
                }
                *generation = *runtime_generation;
            }
            AgentLogKind::TurnStarted {
                turn_sequence,
                origin_key,
                model,
            } => {
                if self.latest_conversation.is_none() || *turn_sequence != entry.seq {
                    return Err("agent turn start is invalid".into());
                }
                if model.as_ref().is_some_and(|model| {
                    model.len() > 256
                        || model.trim() != model
                        || model.chars().any(char::is_control)
                }) {
                    return Err("agent turn model is invalid".into());
                }
                self.transcript
                    .start(*turn_sequence, origin_key.clone(), &entry.appended_at)?;
            }
            AgentLogKind::TurnMetadataSet {
                turn_sequence,
                metadata,
            } => {
                let turn = self
                    .transcript
                    .active
                    .as_mut()
                    .filter(|turn| turn.turn_sequence == *turn_sequence && turn.metadata.is_none())
                    .ok_or_else(|| {
                        "agent turn metadata has no unassigned active turn".to_string()
                    })?;
                if metadata.is_empty()
                    || metadata.len() > 16
                    || metadata
                        .iter()
                        .any(|(key, value)| key.chars().count() > 64 || value.chars().count() > 512)
                {
                    return Err("agent turn metadata is invalid".into());
                }
                turn.metadata = Some(metadata.clone());
            }
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
            } => {
                if *operation_sequence != entry.seq
                    || self
                        .transcript
                        .active
                        .as_ref()
                        .is_none_or(|turn| turn.owner_message.is_none())
                {
                    return Err("agent steering fence has no matching active turn".into());
                }
                self.pending_steering = Some((entry.seq, entry.appended_at.clone()));
            }
            AgentLogKind::OwnerMessage { .. } => {
                let steering = if let Some((sequence, at)) = self.pending_steering.take() {
                    if sequence.checked_add(1) != Some(entry.seq) || at != entry.appended_at {
                        return Err("agent steering fence has no adjacent owner message".into());
                    }
                    true
                } else {
                    false
                };
                self.transcript
                    .owner_message(transcript_message(&entry), steering)?;
            }
            AgentLogKind::AgentMessage { .. } => {
                if self.pending_steering.is_some() {
                    return Err("agent steering fence has no adjacent owner message".into());
                }
                self.transcript.agent_message(transcript_message(&entry))?;
            }
            AgentLogKind::TurnCompleted { turn_sequence } => self.transcript.finish(
                *turn_sequence,
                AgentTurnOutcome::Completed {
                    at: entry.appended_at.clone(),
                },
            )?,
            AgentLogKind::TurnFailed {
                turn_sequence,
                failure,
            } => self.transcript.finish(
                *turn_sequence,
                AgentTurnOutcome::Failed {
                    at: entry.appended_at.clone(),
                    failure: failure.clone(),
                },
            )?,
            AgentLogKind::TurnInterrupted { turn_sequence } => self.transcript.finish(
                *turn_sequence,
                AgentTurnOutcome::Interrupted {
                    at: entry.appended_at.clone(),
                },
            )?,
            AgentLogKind::ConversationGap { frames } => {
                if self.latest_conversation.is_none() || *frames == 0 {
                    return Err("agent conversation gap is invalid".into());
                }
                self.transcript.gap(*frames)?;
            }
            _ => return Ok(()),
        }
        Ok(())
    }

    pub fn finish(mut self, agent: &Agent) -> Result<AgentTranscript, String> {
        if !self.saw_creation || self.pending_steering.is_some() {
            return Err("agent transcript projection is incomplete".into());
        }
        if !matches!(
            (
                agent.conversation.conversation_id.as_deref(),
                agent.conversation.runtime_generation,
                self.latest_conversation.as_ref(),
            ),
            (None, None, None) | (Some(_), Some(_), Some(_))
        ) {
            return Err("agent conversation identity disagrees with its projection".into());
        }
        if let (Some(expected), Some((id, generation))) = (
            agent.conversation.conversation_id.as_deref(),
            self.latest_conversation.as_ref(),
        ) {
            if expected != id || agent.conversation.runtime_generation != Some(*generation) {
                return Err("agent conversation identity disagrees with its projection".into());
            }
        }
        if self
            .transcript
            .active
            .as_ref()
            .map(|turn| turn.turn_sequence)
            != agent.conversation.active_turn
        {
            return Err("agent active turn disagrees with its projection".into());
        }
        self.transcript.validate_queued_messages(agent)?;
        self.transcript.finish_active()?;
        Ok(AgentTranscript {
            agent_id: agent.id.clone(),
            through_seq: agent.conversation.last_seq,
            turns: self.transcript.turns,
        })
    }
}

/// A bounded source page may split a turn. Merge messages by durable sequence;
/// scalar turn metadata repeats, and a changed high-water mark refuses continuation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTranscriptPageRequest {
    pub turn: Option<u64>,
    pub after: Option<u64>,
    pub through: Option<u64>,
    pub limit: u16,
}
impl AgentTranscriptPageRequest {
    pub fn validate(&self) -> Result<(), super::AgentReadError> {
        if self.limit == 0
            || self.limit > 32
            || self.turn.is_some() != self.after.is_some()
            || self.turn.is_some() != self.through.is_some()
            || self.turn == Some(0)
            || self.after.zip(self.through).is_some_and(|(a, b)| a >= b)
        {
            return Err(super::AgentReadError::InvalidPage);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentTranscriptPage {
    pub transcript: AgentTranscript,
    pub next: Option<AgentTranscriptPageRequest>,
}

impl AgentTranscript {
    /// Page a materialized transcript from one verified source snapshot.
    pub fn into_page(
        mut self,
        request: &AgentTranscriptPageRequest,
    ) -> Result<AgentTranscriptPage, super::AgentReadError> {
        request.validate()?;
        if request
            .through
            .is_some_and(|through| through != self.through_seq)
        {
            return Err(super::AgentReadError::InvalidPage);
        }
        let index = match request.turn {
            Some(sequence) => self
                .turns
                .iter()
                .position(|turn| turn.turn_sequence == sequence)
                .ok_or(super::AgentReadError::InvalidPage)?,
            None if self.turns.is_empty() => {
                return Ok(AgentTranscriptPage {
                    transcript: self,
                    next: None,
                })
            }
            None => 0,
        };
        let mut turn = self.turns[index].clone();
        let after = request.after.unwrap_or(turn.turn_sequence - 1);
        let end = self
            .turns
            .get(index + 1)
            .map_or(self.through_seq, |turn| turn.turn_sequence - 1);
        if after < turn.turn_sequence - 1 || after >= end {
            return Err(super::AgentReadError::InvalidPage);
        }
        let through = end.min(after + u64::from(request.limit));
        turn.messages.retain(|message| {
            message.sequence <= through
                && (message.sequence > after
                    || (after == turn.turn_sequence - 1 && message.sequence < turn.turn_sequence))
        });
        turn.agent_messages = turn
            .messages
            .iter()
            .filter(|message| message.role == crate::sandbox::AgentTranscriptRole::Agent)
            .map(|message| message.content.clone())
            .collect();
        let next = if through < end {
            Some((turn.turn_sequence, through))
        } else {
            self.turns
                .get(index + 1)
                .map(|next| (next.turn_sequence, next.turn_sequence - 1))
        }
        .map(|(turn, after)| AgentTranscriptPageRequest {
            turn: Some(turn),
            after: Some(after),
            through: Some(self.through_seq),
            limit: request.limit,
        });
        self.turns = vec![turn];
        Ok(AgentTranscriptPage {
            transcript: self,
            next,
        })
    }
}
