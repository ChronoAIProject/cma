use serde::{Deserialize, Serialize};

use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, AgentStatus};

use super::{AgentCommand, AgentInputOrder, CommandLedgerError, CommandReservation};

const ANSWER_SEQUENCE: &str = "cma.answer_sequence";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerContext {
    pub turn_sequence: u64,
    pub answer_sequence: u64,
}

impl AnswerContext {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        if self.turn_sequence == 0
            || self.answer_sequence <= self.turn_sequence
            || self.answer_sequence > i64::MAX as u64
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }

    pub fn bind(self, command: &mut AgentCommand) -> Result<(), CommandLedgerError> {
        self.validate()?;
        let AgentCommand::Message {
            order, metadata, ..
        } = command
        else {
            return Err(CommandLedgerError::Invalid);
        };
        if metadata.contains_key(ANSWER_SEQUENCE) {
            return Err(CommandLedgerError::Invalid);
        }
        *order = AgentInputOrder::AfterTurn {
            turn_sequence: Some(self.turn_sequence),
        };
        metadata.insert(ANSWER_SEQUENCE.into(), self.answer_sequence.to_string());
        Ok(())
    }

    pub fn from_command(command: &AgentCommand) -> Result<Option<Self>, CommandLedgerError> {
        let AgentCommand::Message {
            order, metadata, ..
        } = command
        else {
            return Ok(None);
        };
        let Some(sequence) = metadata.get(ANSWER_SEQUENCE) else {
            return Ok(None);
        };
        let AgentInputOrder::AfterTurn {
            turn_sequence: Some(turn_sequence),
        } = order
        else {
            return Err(CommandLedgerError::Invalid);
        };
        let answer_sequence = sequence
            .parse::<u64>()
            .map_err(|_| CommandLedgerError::Invalid)?;
        if sequence != &answer_sequence.to_string() {
            return Err(CommandLedgerError::Invalid);
        }
        let context = Self {
            turn_sequence: *turn_sequence,
            answer_sequence,
        };
        context.validate()?;
        Ok(Some(context))
    }
}

#[derive(Default)]
pub struct AnswerObservation {
    turn: Option<u64>,
    message: Option<u64>,
    completed: bool,
}

impl AnswerObservation {
    pub fn observe(&mut self, entry: &AgentLogEntry) {
        match &entry.kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => {
                self.turn = Some(*turn_sequence);
                self.message = None;
                self.completed = false;
            }
            AgentLogKind::OwnerMessage { .. } => {
                self.message = None;
                self.completed = false;
            }
            AgentLogKind::AgentMessage { content } => {
                self.message = (!content.trim().is_empty()).then_some(entry.seq);
                self.completed = false;
            }
            AgentLogKind::TurnCompleted { turn_sequence } => {
                self.completed = self.turn == Some(*turn_sequence);
            }
            AgentLogKind::TurnFailed { .. } | AgentLogKind::TurnInterrupted { .. } => {
                self.completed = false
            }
            _ => {}
        }
    }

    pub fn current(&self) -> Option<AnswerContext> {
        if !self.completed {
            return None;
        }
        Some(AnswerContext {
            turn_sequence: self.turn?,
            answer_sequence: self.message?,
        })
    }
}

pub fn require_answer_context(
    request: &CommandReservation,
    agent: &Agent,
    waiting: u64,
    answer: Option<AnswerContext>,
) -> Result<(), CommandLedgerError> {
    let Some(expected) = AnswerContext::from_command(&request.envelope.command)? else {
        return Ok(());
    };
    if answer != Some(expected)
        || waiting != 0
        || agent.status != AgentStatus::Ready
        || agent.conversation.active_turn.is_some()
        || agent
            .conversation
            .queued_messages
            .iter()
            .any(|message| message.state != crate::sandbox::QueuedMessageState::Delivered)
    {
        return Err(CommandLedgerError::NotAdmissible);
    }
    Ok(())
}
