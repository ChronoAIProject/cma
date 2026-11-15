//! Materialize one turn from certified original header evidence. Stores prove
//! that these are all header events in the captured turn before calling this.
use super::agent_reads_fold::{transcript_message, TranscriptFold};
use super::{Agent, AgentLogEntry, AgentLogKind, AgentLogProjectionState, AgentTranscriptTurn};

pub struct AgentTranscriptWindow {
    pub start: AgentLogEntry,
    pub owner: AgentLogEntry,
    pub metadata: Option<AgentLogEntry>,
    pub terminal: Option<AgentLogEntry>,
    pub gaps: Vec<AgentLogEntry>,
}

impl AgentTranscriptWindow {
    pub fn header(&self, agent: &Agent) -> Result<AgentTranscriptTurn, String> {
        let AgentLogKind::TurnStarted {
            turn_sequence,
            origin_key,
            model,
        } = &self.start.kind
        else {
            return Err("transcript window has no turn start".into());
        };
        let turn = *turn_sequence;
        if turn != self.start.seq
            || model.as_ref().is_some_and(|model| {
                model.len() > 256 || model.trim() != model || model.chars().any(char::is_control)
            })
        {
            return Err("transcript window has an invalid turn start".into());
        }
        for row in std::iter::once(&self.start)
            .chain(std::iter::once(&self.owner))
            .chain(self.metadata.iter())
            .chain(self.terminal.iter())
            .chain(&self.gaps)
        {
            if row.agent_id != agent.id
                || row.seq == 0
                || row.seq > agent.conversation.last_seq
                || crate::seconds_from_rfc3339(&row.appended_at).is_none()
            {
                return Err("transcript window source identity is invalid".into());
            }
        }
        if !matches!(self.owner.kind, AgentLogKind::OwnerMessage { .. }) {
            return Err("transcript window owner evidence is invalid".into());
        }
        let queued = AgentLogProjectionState::queued_owner(&self.start);
        if queued.is_some_and(|seq| seq != self.owner.seq || seq >= turn)
            || (queued.is_none() && self.owner.seq <= turn)
        {
            return Err("transcript window owner sequence is invalid".into());
        }
        let mut fold = TranscriptFold::default();
        if queued.is_some() {
            fold.owner_message(transcript_message(&self.owner), false)?;
        }
        fold.start(turn, origin_key.clone(), &self.start.appended_at)?;
        if queued.is_none() {
            fold.owner_message(transcript_message(&self.owner), false)?;
        }
        let end = self
            .terminal
            .as_ref()
            .map_or(agent.conversation.last_seq.saturating_add(1), |row| row.seq);
        if end <= turn || self.owner.seq >= end {
            return Err("transcript window terminal sequence is invalid".into());
        }
        if let Some(row) = &self.metadata {
            let AgentLogKind::TurnMetadataSet {
                turn_sequence,
                metadata,
            } = &row.kind
            else {
                return Err("transcript window metadata evidence is invalid".into());
            };
            if *turn_sequence != turn
                || row.seq <= turn
                || row.seq >= end
                || metadata.is_empty()
                || metadata.len() > 16
                || metadata
                    .iter()
                    .any(|(k, v)| k.chars().count() > 64 || v.chars().count() > 512)
            {
                return Err("transcript window metadata is invalid".into());
            }
            fold.active.as_mut().unwrap().metadata = Some(metadata.clone());
        }
        let mut previous = turn;
        for row in &self.gaps {
            let AgentLogKind::ConversationGap { frames } = row.kind else {
                return Err("transcript window gap evidence is invalid".into());
            };
            if row.seq <= previous || row.seq >= end || frames == 0 {
                return Err("transcript window gap sequence is invalid".into());
            }
            previous = row.seq;
            fold.gap(frames)?;
        }
        if let Some(row) = &self.terminal {
            use super::AgentTurnOutcome;
            let (sequence, outcome) = match &row.kind {
                AgentLogKind::TurnCompleted { turn_sequence } => (
                    *turn_sequence,
                    AgentTurnOutcome::Completed {
                        at: row.appended_at.clone(),
                    },
                ),
                AgentLogKind::TurnFailed {
                    turn_sequence,
                    failure,
                } => (
                    *turn_sequence,
                    AgentTurnOutcome::Failed {
                        at: row.appended_at.clone(),
                        failure: failure.clone(),
                    },
                ),
                AgentLogKind::TurnInterrupted { turn_sequence } => (
                    *turn_sequence,
                    AgentTurnOutcome::Interrupted {
                        at: row.appended_at.clone(),
                    },
                ),
                _ => return Err("transcript window terminal evidence is invalid".into()),
            };
            if sequence != turn {
                return Err("transcript window terminal belongs to another turn".into());
            }
            fold.finish(turn, outcome)?;
        } else {
            if agent.conversation.active_turn != Some(turn) {
                return Err("transcript window active turn disagrees with Agent".into());
            }
            fold.finish_active()?;
        }
        let mut result = fold.turns.pop().ok_or("transcript window is incomplete")?;
        // The header repeats owner text; the source page decides which message
        // rows are included. A queued owner belongs only to the first window.
        result.messages.retain(|message| message.sequence < turn);
        Ok(result)
    }
}
