use std::collections::BTreeMap;

#[path = "agent_reads_identity.rs"]
mod identity;

use crate::seconds_from_rfc3339;

use super::queued_message::queued_message_origin_sequence;

use super::{
    Agent, AgentLogEntry, AgentLogKind, AgentOperationDelivery, AgentOperationKind,
    AgentTranscript, AgentTranscriptMessage, AgentTranscriptRole, AgentTranscriptTurn,
    AgentTurnOutcome, AgentUsage, AgentUsageReading, TypedFailure,
};

pub(super) struct AgentLogProjection {
    pub(super) transcript: AgentTranscript,
    pub(super) usage: AgentUsageReading,
}

pub(super) fn fold_agent_log(
    agent: &Agent,
    entries: &[AgentLogEntry],
) -> Result<AgentLogProjection, String> {
    let Some(first) = entries.first() else {
        return Err("agent log has no creation record".into());
    };
    match &first.kind {
        AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } if workspace_id == &agent.workspace_id && worktree_id == &agent.worktree_id => {}
        AgentLogKind::Created { .. } => {
            return Err("agent creation log lineage does not match".into());
        }
        _ => return Err("agent log does not begin with creation".into()),
    }

    let mut transcript = TranscriptFold::default();
    let mut usage = UsageFold::default();
    let mut latest_conversation = None;
    for (index, entry) in entries.iter().enumerate() {
        if entry.agent_id != agent.id || entry.seq == 0 {
            return Err("agent log identity or sequence is invalid".into());
        }
        if seconds_from_rfc3339(&entry.appended_at).is_none() {
            return Err("agent log timestamp is invalid".into());
        }
        match &entry.kind {
            AgentLogKind::Created { .. } if entry.seq != 1 => {
                return Err("agent log contains duplicate creation".into());
            }
            AgentLogKind::ConversationOpened {
                conversation_id,
                runtime_generation,
            } => {
                if transcript.active.is_some()
                    || conversation_id.trim().is_empty()
                    || conversation_id.len() > 512
                    || *runtime_generation == 0
                    || latest_conversation
                        .as_ref()
                        .is_some_and(|(_, previous)| runtime_generation <= previous)
                {
                    return Err("agent conversation boundary is invalid".into());
                }
                latest_conversation = Some((conversation_id.clone(), *runtime_generation));
                usage.open_generation(*runtime_generation)?;
            }
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            } => {
                let Some((_, generation)) = latest_conversation.as_mut() else {
                    return Err("agent runtime retirement has no conversation".into());
                };
                if transcript.active.is_some()
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
                if latest_conversation.is_none() || *turn_sequence != entry.seq {
                    return Err("agent turn start is invalid".into());
                }
                if model.as_ref().is_some_and(|model| {
                    model.len() > 256
                        || model.trim() != model
                        || model.chars().any(char::is_control)
                }) {
                    return Err("agent turn model is invalid".into());
                }
                transcript.start(*turn_sequence, origin_key.clone(), &entry.appended_at)?;
            }
            AgentLogKind::TurnMetadataSet {
                turn_sequence,
                metadata,
            } => {
                let turn = transcript
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
            } if *operation_sequence != entry.seq
                || transcript
                    .active
                    .as_ref()
                    .is_none_or(|turn| turn.owner_message.is_none())
                || !entries.get(index + 1).is_some_and(|next| {
                    entry.seq.checked_add(1) == Some(next.seq)
                        && entry.appended_at == next.appended_at
                        && matches!(next.kind, AgentLogKind::OwnerMessage { .. })
                }) =>
            {
                return Err("agent steering fence has no matching active-turn input".into());
            }
            AgentLogKind::OwnerMessage { .. } => {
                let steering = index.checked_sub(1).is_some_and(|previous| {
                    matches!(
                        entries[previous].kind,
                        AgentLogKind::OperationDeliveryChanged {
                            operation: AgentOperationKind::Steer,
                            state: AgentOperationDelivery::DispatchFenced,
                            ..
                        }
                    )
                });
                transcript.owner_message(transcript_message(entry), steering)?
            }
            AgentLogKind::AgentMessage { .. } => {
                transcript.agent_message(transcript_message(entry))?
            }
            AgentLogKind::TurnCompleted { turn_sequence } => transcript.finish(
                *turn_sequence,
                AgentTurnOutcome::Completed {
                    at: entry.appended_at.clone(),
                },
            )?,
            AgentLogKind::TurnFailed {
                turn_sequence,
                failure,
            } => transcript.finish(
                *turn_sequence,
                AgentTurnOutcome::Failed {
                    at: entry.appended_at.clone(),
                    failure: failure.clone(),
                },
            )?,
            AgentLogKind::TurnInterrupted { turn_sequence } => transcript.finish(
                *turn_sequence,
                AgentTurnOutcome::Interrupted {
                    at: entry.appended_at.clone(),
                },
            )?,
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            } => usage.observe(
                *input_tokens,
                *output_tokens,
                *context_used,
                *context_window,
            )?,
            AgentLogKind::ConversationGap { frames } => {
                if latest_conversation.is_none() || *frames == 0 {
                    return Err("agent conversation gap is invalid".into());
                }
                transcript.gap(*frames)?;
            }
            _ => {}
        }
    }

    identity::validate(agent, latest_conversation.as_ref(), entries)?;
    let active_turn = transcript.active.as_ref().map(|turn| turn.turn_sequence);
    if active_turn != agent.conversation.active_turn {
        return Err("agent active turn disagrees with its log".into());
    }
    transcript.validate_queued_messages(agent)?;
    transcript.finish_active()?;

    Ok(AgentLogProjection {
        transcript: AgentTranscript {
            agent_id: agent.id.clone(),
            through_seq: agent.conversation.last_seq,
            turns: transcript.turns,
        },
        usage: AgentUsageReading {
            agent_id: agent.id.clone(),
            through_seq: agent.conversation.last_seq,
            usage: usage.reading(),
        },
    })
}

#[derive(Default)]
struct TranscriptFold {
    turns: Vec<AgentTranscriptTurn>,
    active: Option<ActiveTurn>,
    queued_owner_messages: BTreeMap<u64, QueuedOwnerMessage>,
}

struct QueuedOwnerMessage {
    message: AgentTranscriptMessage,
    used: bool,
}

impl TranscriptFold {
    fn start(
        &mut self,
        turn_sequence: u64,
        origin_key: Option<String>,
        started_at: &str,
    ) -> Result<(), String> {
        if self.active.is_some()
            || turn_sequence == 0
            || self
                .turns
                .last()
                .is_some_and(|turn| turn.turn_sequence >= turn_sequence)
        {
            return Err("agent turns are nested or out of order".into());
        }
        let owner_message = match origin_key
            .as_deref()
            .and_then(queued_message_origin_sequence)
        {
            Some(sequence) => {
                let queued = self
                    .queued_owner_messages
                    .get_mut(&sequence)
                    .ok_or_else(|| {
                        "queued turn has no matching admitted owner message".to_string()
                    })?;
                queued.used = true;
                Some(queued.message.clone())
            }
            None => None,
        };
        self.active = Some(ActiveTurn {
            turn_sequence,
            started_at: started_at.into(),
            origin_key,
            owner_message: owner_message
                .as_ref()
                .map(|message| message.content.clone()),
            messages: owner_message.into_iter().collect(),
            agent_messages: Vec::new(),
            metadata: None,
            lost_frames: 0,
        });
        Ok(())
    }

    fn owner_message(
        &mut self,
        message: AgentTranscriptMessage,
        steering: bool,
    ) -> Result<(), String> {
        if let Some(turn) = self.active.as_mut() {
            if !steering && (turn.owner_message.is_some() || !turn.agent_messages.is_empty()) {
                return Err("agent turn has an invalid owner message".into());
            }
            if !steering {
                turn.owner_message = Some(message.content.clone());
            }
            turn.messages.push(message);
            return Ok(());
        }
        self.queued_owner_messages
            .insert(
                message.sequence,
                QueuedOwnerMessage {
                    message,
                    used: false,
                },
            )
            .is_none()
            .then_some(())
            .ok_or_else(|| "agent log contains a duplicate queued owner message".into())
    }

    fn validate_queued_messages(&self, agent: &Agent) -> Result<(), String> {
        let queued = agent
            .conversation
            .queued_messages
            .iter()
            .filter(|message| message.state == super::QueuedMessageState::Queued)
            .map(|message| (message.sequence, message.content.as_str()))
            .collect::<BTreeMap<_, _>>();
        let every_unconsumed_is_queued =
            self.queued_owner_messages
                .iter()
                .all(|(sequence, message)| {
                    message.used || queued.get(sequence) == Some(&message.message.content.as_str())
                });
        let every_queued_message_was_admitted = queued.iter().all(|(sequence, content)| {
            self.queued_owner_messages
                .get(sequence)
                .is_some_and(|message| message.message.content == *content)
        });
        (every_unconsumed_is_queued && every_queued_message_was_admitted)
            .then_some(())
            .ok_or_else(|| "queued owner messages disagree with durable queue state".into())
    }

    fn agent_message(&mut self, message: AgentTranscriptMessage) -> Result<(), String> {
        let turn = self
            .active
            .as_mut()
            .ok_or_else(|| "agent message is outside a turn".to_string())?;
        if turn.owner_message.is_none() {
            return Err("agent message precedes its owner message".into());
        }
        turn.agent_messages.push(message.content.clone());
        turn.messages.push(message);
        Ok(())
    }

    fn gap(&mut self, frames: u64) -> Result<(), String> {
        // A declared transport gap does not invalidate the surrounding durable
        // log. Preserve it for event readers and scope incomplete output to the
        // active turn; an idle gap cannot fail a later, independent turn.
        if let Some(turn) = self.active.as_mut() {
            turn.lost_frames = turn
                .lost_frames
                .checked_add(frames)
                .ok_or_else(|| "agent conversation gap count overflowed".to_string())?;
        }
        Ok(())
    }

    fn finish(&mut self, turn_sequence: u64, outcome: AgentTurnOutcome) -> Result<(), String> {
        let turn = self
            .active
            .take()
            .ok_or_else(|| "agent turn outcome is outside a turn".to_string())?;
        if turn.turn_sequence != turn_sequence {
            return Err("agent turn outcome sequence does not match".into());
        }
        self.turns.push(turn.into_transcript(outcome)?);
        Ok(())
    }

    fn finish_active(&mut self) -> Result<(), String> {
        if let Some(turn) = self.active.take() {
            self.turns
                .push(turn.into_transcript(AgentTurnOutcome::Active)?);
        }
        Ok(())
    }
}

struct ActiveTurn {
    turn_sequence: u64,
    started_at: String,
    origin_key: Option<String>,
    owner_message: Option<String>,
    agent_messages: Vec<String>,
    messages: Vec<AgentTranscriptMessage>,
    metadata: Option<BTreeMap<String, String>>,
    lost_frames: u64,
}

impl ActiveTurn {
    fn into_transcript(self, outcome: AgentTurnOutcome) -> Result<AgentTranscriptTurn, String> {
        let outcome = match outcome {
            AgentTurnOutcome::Completed { at } if self.lost_frames > 0 => {
                AgentTurnOutcome::Failed {
                    at,
                    failure: TypedFailure {
                        class: "conversation_output_gap".into(),
                        detail: format!(
                            "{} runtime output frames were lost. Recorded messages are preserved, but this output is incomplete.",
                            self.lost_frames
                        ),
                    },
                }
            }
            outcome => outcome,
        };
        Ok(AgentTranscriptTurn {
            turn_sequence: self.turn_sequence,
            started_at: self.started_at,
            origin_key: self.origin_key,
            owner_message: self
                .owner_message
                .ok_or_else(|| "agent turn has no owner message".to_string())?,
            agent_messages: self.agent_messages,
            messages: self.messages,
            metadata: self.metadata.unwrap_or_default(),
            outcome,
        })
    }
}

fn transcript_message(entry: &AgentLogEntry) -> AgentTranscriptMessage {
    let (role, content) = match &entry.kind {
        AgentLogKind::OwnerMessage { content } => (AgentTranscriptRole::Owner, content),
        AgentLogKind::AgentMessage { content } => (AgentTranscriptRole::Agent, content),
        _ => unreachable!("only message entries become transcript messages"),
    };
    AgentTranscriptMessage {
        sequence: entry.seq,
        role,
        content: content.clone(),
        appended_at: entry.appended_at.clone(),
    }
}

#[derive(Default)]
struct UsageFold {
    closed_input: u64,
    closed_output: u64,
    generation: Option<u64>,
    input_peak: u64,
    output_peak: u64,
    context_used: u64,
    context_window: Option<u64>,
}

impl UsageFold {
    fn open_generation(&mut self, generation: u64) -> Result<(), String> {
        if self.generation.is_some_and(|current| generation <= current) {
            return Err("agent usage generation is not increasing".into());
        }
        if self.generation.is_some() {
            self.closed_input = self.closed_input.saturating_add(self.input_peak);
            self.closed_output = self.closed_output.saturating_add(self.output_peak);
        }
        self.generation = Some(generation);
        self.input_peak = 0;
        self.output_peak = 0;
        self.context_used = 0;
        self.context_window = None;
        Ok(())
    }

    fn observe(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        context_used: u64,
        context_window: Option<u64>,
    ) -> Result<(), String> {
        if self.generation.is_none() {
            return Err("agent usage precedes its conversation".into());
        }
        self.input_peak = self.input_peak.max(input_tokens);
        self.output_peak = self.output_peak.max(output_tokens);
        if context_used > 0 {
            self.context_used = context_used;
        }
        if let Some(context_window) = context_window {
            self.context_window = Some(context_window);
        }
        Ok(())
    }

    fn reading(&self) -> AgentUsage {
        AgentUsage {
            input_tokens: self.closed_input.saturating_add(self.input_peak),
            output_tokens: self.closed_output.saturating_add(self.output_peak),
            context_used: self.context_used,
            context_window: self.context_window,
        }
    }
}
