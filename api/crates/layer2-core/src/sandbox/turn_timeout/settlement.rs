use sha2::{Digest, Sha256};

use super::TurnTimeoutError;
use crate::sandbox::{
    agent_io::{
        AdmittedAgentCommand, AgentCommand, AgentResponseStatus, CommandFailure,
        CommandFailureCode, CommandLane, CommandObservation,
    },
    conversation_fold::{changed, FoldedConversationUpdate},
    conversation_fold_support::{fail_first_message, flush_message},
    store::{AgentConversationMutation, AgentMessageCheckpoint, AgentMessageSnapshot},
    AgentLogEntry, AgentLogKind, AgentMessageAssembly, AgentStatus, AgentUpdateOutcome,
};

/// A service-policy publication, with no synthetic runtime update or ACK.
/// Stores compare the snapshot and original command in the same transaction
/// that writes this mutation, checkpoint, command/lane and durable action receipt.
pub struct TurnTimeoutSettlement {
    snapshot: AgentMessageSnapshot,
    command: AdmittedAgentCommand,
    mutation: AgentConversationMutation,
    checkpoint: AgentMessageCheckpoint,
}

impl TurnTimeoutSettlement {
    pub fn prepare(
        snapshot: AgentMessageSnapshot,
        command: AdmittedAgentCommand,
        expected_revision: u64,
        appended_at: String,
    ) -> Result<Self, TurnTimeoutError> {
        let now_ms = time::OffsetDateTime::parse(
            &appended_at,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()
        .and_then(|value| i64::try_from(value.unix_timestamp_nanos() / 1_000_000).ok())
        .ok_or(TurnTimeoutError::Invalid)?;
        let current = &snapshot.agent;
        current
            .validate_conversation()
            .map_err(|_| TurnTimeoutError::Invalid)?;
        command.validate().map_err(|_| TurnTimeoutError::Invalid)?;
        let timeout = current
            .conversation
            .turn_timeout
            .as_ref()
            .ok_or(TurnTimeoutError::Conflict)?;
        let target = timeout.binding();
        let dispatch = command
            .work
            .dispatch
            .as_ref()
            .ok_or(TurnTimeoutError::Conflict)?;
        if !target.matches_active_turn(current)
            || command.status.is_terminal()
            || !matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
            || command.receipt.envelope.agent_id != current.id
            || command.receipt.envelope.command_id != target.command_id
            || dispatch.claim.agent_id != target.agent_id
            || dispatch.claim.command_id != target.command_id
            || dispatch.turn_sequence != target.turn_sequence
            || dispatch.operation_sequence != target.operation_sequence
            || dispatch.runtime_generation != target.runtime_generation
        {
            return Err(TurnTimeoutError::Conflict);
        }
        if let Some(previous) = &snapshot.checkpoint {
            previous.validate().map_err(|_| TurnTimeoutError::Invalid)?;
            if !previous.matches(current) {
                return Err(TurnTimeoutError::Conflict);
            }
        }
        let assembly = snapshot
            .checkpoint
            .as_ref()
            .map(|previous| previous.assembly.clone())
            .unwrap_or_default();
        let terminal = timeout.expire(expected_revision, now_ms)?;
        let approval_events = terminal.approval_events_since(timeout)?;
        let mut agent = current.clone();
        let failure = terminal.cause().failure();
        agent.conversation.turn_timeout = Some(terminal);
        let mut logs = flush_message(&assembly);
        fail_first_message(
            &mut agent,
            &failure.class,
            &failure.detail,
            &appended_at,
            &mut logs,
        )
        .map_err(|_| TurnTimeoutError::Conflict)?;
        logs.extend(approval_events);
        logs.push(AgentLogKind::TurnFailed {
            turn_sequence: target.turn_sequence,
            failure,
        });
        logs.push(AgentLogKind::StatusChanged {
            from: current.status,
            to: AgentStatus::Ready,
        });
        let FoldedConversationUpdate::Changed(change) = changed(
            agent,
            logs,
            AgentMessageAssembly::default(),
            Some(AgentStatus::Ready),
            None,
            true,
            &appended_at,
        )
        .map_err(|_| TurnTimeoutError::Conflict)?
        else {
            return Err(TurnTimeoutError::Conflict);
        };
        let mutation = AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: target.runtime_generation,
            agent: change.agent,
            appended_at: appended_at.clone(),
            agent_logs: change.logs,
        };
        let fingerprint = serde_json::to_vec(&(timeout.action_id(), target, timeout.cause()))
            .map_err(|_| TurnTimeoutError::Invalid)?;
        let checkpoint = AgentMessageCheckpoint {
            version: 1,
            update_id: timeout.action_id().into(),
            update_fingerprint: format!("{:x}", Sha256::digest(fingerprint)),
            native_conversation_id: target.conversation_id.clone(),
            runtime_generation: target.runtime_generation,
            active_turn: None,
            after_seq: mutation.agent.conversation.last_seq,
            assembly: change.assembly,
            outcome: AgentUpdateOutcome {
                turn_closed: true,
                appended: mutation
                    .agent_logs
                    .iter()
                    .enumerate()
                    .map(|(offset, kind)| AgentLogEntry {
                        agent_id: current.id.clone(),
                        seq: current.conversation.last_seq + offset as u64 + 1,
                        appended_at: appended_at.clone(),
                        kind: kind.clone(),
                    })
                    .collect(),
            },
            output: snapshot
                .checkpoint
                .as_ref()
                .and_then(|previous| previous.output.clone()),
        };
        checkpoint
            .validate()
            .map_err(|_| TurnTimeoutError::Invalid)?;
        Ok(Self {
            snapshot,
            command,
            mutation,
            checkpoint,
        })
    }

    pub fn snapshot(&self) -> &AgentMessageSnapshot {
        &self.snapshot
    }
    pub fn command(&self) -> &AdmittedAgentCommand {
        &self.command
    }
    pub fn mutation(&self) -> &AgentConversationMutation {
        &self.mutation
    }
    pub fn checkpoint(&self) -> &AgentMessageCheckpoint {
        &self.checkpoint
    }

    /// Preserve delivery knowledge; local policy failure is not non-dispatch.
    /// A successor's lane lease is never released by this command's terminal.
    pub fn settle_command(
        &self,
        lane: &mut CommandLane,
    ) -> Result<AdmittedAgentCommand, TurnTimeoutError> {
        let mut next = self.command.clone();
        let dispatch = next
            .work
            .dispatch
            .as_ref()
            .ok_or(TurnTimeoutError::Conflict)?;
        let delivery = next
            .work
            .observation
            .as_ref()
            .map_or(dispatch.delivery, |value| value.delivery);
        if lane
            .lease
            .as_ref()
            .is_some_and(|lease| lease.claim == dispatch.claim)
        {
            lane.lease = None;
        }
        next.status = AgentResponseStatus::Failed;
        next.work.failure = Some(CommandFailure {
            code: CommandFailureCode::ExecutionFailed,
            at: self.mutation.appended_at.clone(),
        });
        next.work.observation = Some(CommandObservation {
            event_cursor: self.checkpoint.after_seq,
            delivery,
        });
        next.work.lease = None;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(TurnTimeoutError::Invalid)?;
        next.validate().map_err(|_| TurnTimeoutError::Invalid)?;
        Ok(next)
    }
}
