use std::collections::BTreeSet;

use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, AgentOperationDelivery};
use crate::seconds_from_rfc3339;

use super::{
    AdmittedAgentCommand, AgentCommand, AgentResponseStatus, CommandDispatch, CommandFailure,
    CommandFailureCode, CommandLane, CommandLedgerError, CommandObservation,
};

mod certified;
pub use certified::CertifiedCommandSource;

/// A bounded streaming fold of an Agent's complete durable log. The log is the
/// recovery source after a worker crash; live fragments never establish outcome.
pub struct CommandEvidence {
    dispatch: CommandDispatch,
    message: bool,
    trigger_first: bool,
    kind: crate::sandbox::AgentOperationKind,
    cursor: u64,
    turn: Option<u64>,
    fence_seen: bool,
    turn_seen: bool,
    delivery: AgentOperationDelivery,
    status: AgentResponseStatus,
    failure: Option<CommandFailure>,
    approvals: BTreeSet<String>,
    prewrite_marker: Option<(u64, String)>,
    deferred_expected: Option<String>,
    deferred: Option<(u64, String)>,
    sealed: bool,
}

impl CommandEvidence {
    /// Begin at an ordinary message's exact dispatch fence after the store has
    /// independently proved the immutable prefix through trusted integrity
    /// metadata. Before this fence, message evidence observes no delivery,
    /// approvals or outcome; its own TurnStarted follows the fence. Controls
    /// need the preceding active turn and must use the complete fold.
    pub fn from_trusted_message_prefix(
        command: &AdmittedAgentCommand,
    ) -> Result<Self, CommandLedgerError> {
        let mut evidence = Self::new(command)?;
        if !evidence.message
            || evidence.trigger_first
            || evidence.dispatch.operation_sequence.checked_add(1)
                != Some(evidence.dispatch.turn_sequence)
        {
            return Err(CommandLedgerError::Invalid);
        }
        evidence.cursor = evidence
            .dispatch
            .operation_sequence
            .checked_sub(1)
            .ok_or(CommandLedgerError::Corrupt)?;
        Ok(evidence)
    }

    pub fn new(command: &AdmittedAgentCommand) -> Result<Self, CommandLedgerError> {
        command.validate()?;
        let dispatch = command
            .work
            .dispatch
            .clone()
            .ok_or(CommandLedgerError::Conflict)?;
        Ok(Self {
            sealed: false,
            deferred_expected: command.receipt.envelope.continuation.as_ref().map(|_| {
                super::continuation_retry::deferred_detail(
                    &command.receipt.envelope,
                    dispatch.operation_sequence,
                    dispatch.turn_sequence,
                )
            }),
            deferred: None,
            prewrite_marker: None,
            dispatch,
            trigger_first: matches!(
                command.receipt.envelope.actor,
                super::AgentActor::Trigger { .. }
            ),
            message: matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            ),
            kind: command.receipt.envelope.command.kind(),
            cursor: 0,
            turn: None,
            fence_seen: false,
            turn_seen: false,
            delivery: AgentOperationDelivery::DispatchFenced,
            status: AgentResponseStatus::InProgress,
            failure: None,
            approvals: BTreeSet::new(),
        })
    }

    pub fn observe(&mut self, entry: &AgentLogEntry) -> Result<(), CommandLedgerError> {
        if self.sealed
            || entry.agent_id != self.dispatch.claim.agent_id
            || self.cursor.checked_add(1) != Some(entry.seq)
            || seconds_from_rfc3339(&entry.appended_at).is_none()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        self.cursor = entry.seq;
        if let AgentLogKind::TurnStarted {
            turn_sequence,
            origin_key,
            ..
        } = &entry.kind
        {
            if *turn_sequence != entry.seq {
                return Err(CommandLedgerError::Corrupt);
            }
            self.turn = Some(*turn_sequence);
            if *turn_sequence == self.dispatch.turn_sequence {
                if self.message
                    && origin_key.as_deref()
                        != Some(&format!(
                            "command:{}",
                            self.dispatch.claim.command_id.as_str()
                        ))
                {
                    return Err(CommandLedgerError::Corrupt);
                }
                self.turn_seen = true;
            }
        }
        if entry.seq == self.dispatch.operation_sequence {
            if entry.appended_at != self.dispatch.fenced_at
                || entry.kind
                    != (AgentLogKind::OperationDeliveryChanged {
                        operation_sequence: self.dispatch.operation_sequence,
                        operation: self.kind,
                        state: AgentOperationDelivery::DispatchFenced,
                    })
                || (!self.message && self.turn != Some(self.dispatch.turn_sequence))
            {
                return Err(CommandLedgerError::Corrupt);
            }
            self.fence_seen = true;
        }
        if !self.fence_seen {
            return Ok(());
        }
        match &entry.kind {
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation,
                state,
            } if *operation_sequence == self.dispatch.operation_sequence => {
                if *operation != self.kind {
                    return Err(CommandLedgerError::Corrupt);
                }
                self.delivery(*state, &entry.appended_at)?;
                if *state == AgentOperationDelivery::PreWriteFailed {
                    self.prewrite_marker = Some((entry.seq, entry.appended_at.clone()));
                }
            }
            AgentLogKind::TurnCompleted { turn_sequence }
                if self.message && *turn_sequence == self.dispatch.turn_sequence =>
            {
                self.finish(AgentResponseStatus::Completed, None)?;
            }
            AgentLogKind::TurnInterrupted { turn_sequence }
                if self.message && *turn_sequence == self.dispatch.turn_sequence =>
            {
                self.finish(AgentResponseStatus::Cancelled, None)?;
            }
            AgentLogKind::TurnFailed {
                turn_sequence,
                failure,
            } if self.message && *turn_sequence == self.dispatch.turn_sequence => {
                if failure.class == super::SOURCE_DEFERRED_FAILURE
                    && self.delivery == AgentOperationDelivery::PreWriteFailed
                    && self.prewrite_marker.as_ref().is_some_and(|(seq, at)| {
                        seq.checked_add(1) == Some(entry.seq) && at == &entry.appended_at
                    })
                    && self.deferred_expected.as_ref() == Some(&failure.detail)
                {
                    self.deferred = Some((entry.seq, entry.appended_at.clone()));
                }
                self.finish(
                    AgentResponseStatus::Failed,
                    Some(CommandFailure {
                        code: self
                            .failure
                            .as_ref()
                            .map_or(CommandFailureCode::ExecutionFailed, |failure| failure.code),
                        at: entry.appended_at.clone(),
                    }),
                )?;
            }
            AgentLogKind::ApprovalRequested { approval_id, .. } if self.in_message_turn() => {
                self.approvals.insert(approval_id.clone());
                self.status = AgentResponseStatus::WaitingAction;
            }
            AgentLogKind::ApprovalResolved { approval_id, .. } if self.in_message_turn() => {
                self.approvals.remove(approval_id);
                if self.approvals.is_empty() {
                    self.status = AgentResponseStatus::InProgress;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn in_message_turn(&self) -> bool {
        self.message && !self.status.is_terminal() && self.turn == Some(self.dispatch.turn_sequence)
    }

    fn delivery(
        &mut self,
        next: AgentOperationDelivery,
        at: &str,
    ) -> Result<(), CommandLedgerError> {
        use AgentOperationDelivery::*;
        if self.delivery != next
            && !matches!(
                (self.delivery, next),
                (
                    DispatchFenced,
                    WriteConfirmed
                        | ExecutionObserved
                        | PreWriteFailed
                        | RecoveryRequired
                        | RuntimeLost
                ) | (
                    RecoveryRequired,
                    // Explicit, proof-checked operator recovery can establish
                    // that a legacy ambiguous prompt never reached the writer.
                    WriteConfirmed | ExecutionObserved | PreWriteFailed | RuntimeLost
                ) | (WriteConfirmed, ExecutionObserved)
            )
        {
            return Err(CommandLedgerError::Corrupt);
        }
        self.delivery = next;
        match next {
            WriteConfirmed | ExecutionObserved if !self.message => {
                self.finish(AgentResponseStatus::Completed, None)?
            }
            RecoveryRequired if self.trigger_first && !self.status.is_terminal() => {
                self.status = AgentResponseStatus::WaitingAction;
            }
            WriteConfirmed | ExecutionObserved
                if self.trigger_first
                    && !self.status.is_terminal()
                    && self.approvals.is_empty() =>
            {
                self.status = AgentResponseStatus::InProgress;
            }
            PreWriteFailed | RuntimeLost => self.finish(
                AgentResponseStatus::Failed,
                Some(CommandFailure {
                    code: if next == PreWriteFailed {
                        CommandFailureCode::RuntimeUnavailable
                    } else {
                        CommandFailureCode::RuntimeLost
                    },
                    at: at.into(),
                }),
            )?,
            _ => {}
        }
        Ok(())
    }

    fn finish(
        &mut self,
        status: AgentResponseStatus,
        failure: Option<CommandFailure>,
    ) -> Result<(), CommandLedgerError> {
        // Runtime retirement records interruption before the unresolved write
        // becomes RuntimeLost. Preserve that stronger uncertainty in either order.
        let lost_write = |failure: &Option<CommandFailure>| {
            failure
                .as_ref()
                .is_some_and(|failure| failure.code == CommandFailureCode::RuntimeLost)
        };
        if self.status == AgentResponseStatus::Failed
            && lost_write(&self.failure)
            && status == AgentResponseStatus::Cancelled
        {
            return Ok(());
        }
        let lost_interrupted_write = self.status == AgentResponseStatus::Cancelled
            && status == AgentResponseStatus::Failed
            && lost_write(&failure);
        if self.status.is_terminal() && self.status != status && !lost_interrupted_write {
            return Err(CommandLedgerError::Corrupt);
        }
        self.status = status;
        if self.failure.is_none() {
            self.failure = failure;
        }
        Ok(())
    }

    /// Inspect only evidence that has been validated through this Agent snapshot.
    /// Does not infer a terminal from elapsed time, runtime loss or Agent state,
    /// and never changes the command or lane. A deferred source attempt is not
    /// the logical message's terminal and requires separate retry assessment.
    pub fn verified_status(
        &self,
        command: &AdmittedAgentCommand,
        agent: &Agent,
    ) -> Result<AgentResponseStatus, CommandLedgerError> {
        command.validate()?;
        if agent.id != self.dispatch.claim.agent_id
            || agent.conversation.last_seq != self.cursor
            || !self.fence_seen
            || !self.turn_seen
            || command.work.dispatch.as_ref() != Some(&self.dispatch)
            || command
                .work
                .observation
                .as_ref()
                .is_some_and(|observation| observation.event_cursor > self.cursor)
            || self.has_source_deferred()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(self.status)
    }

    /// Apply only after the snapshot's entire log was validated. Clear exactly
    /// this command's lease; a newer command may already own the Agent lane.
    pub fn apply(
        self,
        command: &mut AdmittedAgentCommand,
        lane: &mut CommandLane,
        agent: &Agent,
    ) -> Result<bool, CommandLedgerError> {
        self.apply_with_continuation_retry(command, lane, agent, false)
    }
    pub fn has_source_deferred(&self) -> bool {
        self.deferred.is_some()
    }
    pub fn apply_with_continuation_retry(
        mut self,
        command: &mut AdmittedAgentCommand,
        lane: &mut CommandLane,
        agent: &Agent,
        retry_authority_current: bool,
    ) -> Result<bool, CommandLedgerError> {
        if agent.id != self.dispatch.claim.agent_id
            || agent.conversation.last_seq != self.cursor
            || !self.fence_seen
            || !self.turn_seen
            || command.work.dispatch.as_ref() != Some(&self.dispatch)
            || command
                .work
                .observation
                .as_ref()
                .is_some_and(|observation| observation.event_cursor > self.cursor)
        {
            return Err(CommandLedgerError::Corrupt);
        }
        if !self.status.is_terminal()
            && (!agent.status.is_live()
                || agent.conversation.runtime_generation != Some(self.dispatch.runtime_generation))
        {
            self.finish(
                AgentResponseStatus::Failed,
                Some(CommandFailure {
                    code: CommandFailureCode::RuntimeLost,
                    at: agent.updated_at.clone(),
                }),
            )?;
        }
        if !self.status.is_terminal()
            && self.message
            && agent.conversation.active_turn != Some(self.dispatch.turn_sequence)
        {
            return Err(CommandLedgerError::Corrupt);
        }
        // The same aggregate snapshot carries external waits; native approval
        // indexes and native action selectors keep their original namespace.
        if self.message
            && !self.status.is_terminal()
            && agent
                .conversation
                .turn_timeout
                .as_ref()
                .is_some_and(|policy| {
                    policy.binding().matches_active_turn(agent)
                        && policy.binding().command_id == self.dispatch.claim.command_id
                        && policy.binding().operation_sequence == self.dispatch.operation_sequence
                        && policy.is_waiting_for_external_approval()
                })
        {
            self.status = AgentResponseStatus::WaitingAction;
        }
        let mut next = command.clone();
        next.status = self.status;
        next.work.failure = self.failure;
        next.work.observation = Some(CommandObservation {
            event_cursor: self.cursor,
            delivery: self.delivery,
        });
        if self.status.is_terminal()
            || matches!(
                self.delivery,
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            )
        {
            next.work.lease = None;
            if lane
                .lease
                .as_ref()
                .is_some_and(|lease| lease.claim == self.dispatch.claim)
            {
                lane.lease = None;
            }
        }
        if let Some((cursor, at)) = &self.deferred {
            next.defer_source_attempt(agent, retry_authority_current, *cursor, at)?;
        }
        if &next == command {
            return Ok(false);
        }
        next.revision = command
            .revision
            .checked_add(1)
            .ok_or(CommandLedgerError::Corrupt)?;
        next.validate()?;
        *command = next;
        Ok(true)
    }
}
