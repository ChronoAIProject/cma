//! A finite terminal settlement after a known-aborted conversation append.
use super::{
    AgentConversationMutation, AgentMessageCheckpoint, SandboxStoreError,
    AGENT_LOG_CAPACITY_FAILURE,
};
use crate::sandbox::conversation_fold::{fold_update, FoldedConversationUpdate};
use crate::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentMessageAssembly, AgentStatus, AgentWorkStep,
    TypedFailure,
};
use layer3_adapter::{ConversationUpdate, PromptIdentity};

/// Opaque recorder proof. Stores must recheck the exact snapshot and the rejected
/// append's normal capacity before spending the stop/failure reserve.
#[derive(Clone, Debug)]
pub struct AgentConversationCapacitySettlement {
    current: Agent,
    previous: Option<AgentMessageCheckpoint>,
    next: AgentMessageCheckpoint,
    mutation: AgentConversationMutation,
    refused: Vec<AgentLogEntry>,
    original_mutation: AgentConversationMutation,
    original_checkpoint: AgentMessageCheckpoint,
    compact: bool,
}

impl AgentConversationCapacitySettlement {
    pub(crate) fn new(
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        attempted: AgentMessageCheckpoint,
        attempted_mutation: AgentConversationMutation,
        identified_terminal: bool,
    ) -> Result<Self, SandboxStoreError> {
        let turn = current
            .conversation
            .active_turn
            .ok_or(SandboxStoreError::AgentLogCapacityExceeded)?;
        if !matches!(
            current.status,
            AgentStatus::Working | AgentStatus::WaitingApproval
        ) || current.failure.is_some()
            || current.requested_stop_reason.is_some()
            || current.work.cleanup_not_before.is_some()
            || !matches!(
                current.work.step,
                AgentWorkStep::None
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::DispatchFirstMessage
            )
            || previous
                .as_ref()
                .is_some_and(|value| !value.matches(&current) && !value.assembly.is_empty())
            || attempted.outcome.appended.is_empty()
        {
            return Err(SandboxStoreError::AgentLogCapacityExceeded);
        }
        let original_mutation = attempted_mutation.clone();
        let original_checkpoint = attempted.clone();
        let at = attempted_mutation.appended_at.clone();
        let mut failure = TypedFailure {
            class: AGENT_LOG_CAPACITY_FAILURE.into(),
            detail: "Durable Agent history reached its lifetime budget. Retained output and runtime identity are preserved. This Agent cannot continue under the current limits; inspect capacity before explicit recovery or replacement.".into(),
        };
        let native_terminal = attempted.outcome.appended.len() <= super::MAX_AGENT_LOG_BATCH_SIZE
            && identified_terminal
            && attempted.outcome.turn_closed
            && attempted_mutation.agent.status == AgentStatus::Ready
            && attempted_mutation
                .agent_logs
                .iter()
                .filter(|kind| {
                    matches!(kind,
                        AgentLogKind::TurnCompleted { turn_sequence }
                        | AgentLogKind::TurnInterrupted { turn_sequence }
                        | AgentLogKind::TurnFailed { turn_sequence, .. } if *turn_sequence == turn
                    )
                })
                .count()
                == 1;
        let mut mutation = if native_terminal {
            attempted_mutation
        } else {
            let prefix = AgentMessageAssembly::default();
            if previous
                .as_ref()
                .is_some_and(|value| !value.assembly.is_empty())
            {
                failure.detail.push_str(" The unflushed assistant prefix is retained in the private capacity checkpoint for operator export; it was not published as a completed answer.");
            }
            // Capacity is not evidence that an ambiguous input reached native
            // execution. Preserve its delivery facts while settling this turn.
            let update = ConversationUpdate::WithoutPromptEvidence(Box::new(
                ConversationUpdate::FailedForPrompt {
                    prompt_id: PromptIdentity::new(
                        current
                            .conversation
                            .conversation_id
                            .as_deref()
                            .ok_or(SandboxStoreError::Conflict)?,
                        attempted.runtime_generation,
                        turn,
                    ),
                    message: failure.detail.clone(),
                    capability: Some(AGENT_LOG_CAPACITY_FAILURE.into()),
                },
            ));
            let FoldedConversationUpdate::Changed(change) =
                fold_update(&current, &prefix, update, &at, true)
                    .map_err(|_| SandboxStoreError::Conflict)?
            else {
                return Err(SandboxStoreError::Conflict);
            };
            let mut agent = change.agent;
            // Consumption is authoritative even when its enclosing append was
            // refused. Do not carry unrelated metadata or delivery changes.
            agent.conversation.observations.usage = original_mutation
                .agent
                .conversation
                .observations
                .usage
                .clone();
            let mut logs: Vec<_> = original_mutation
                .agent_logs
                .iter()
                .filter(|kind| {
                    matches!(
                        kind,
                        AgentLogKind::Usage { .. } | AgentLogKind::UsageUpdated { .. }
                    )
                })
                .cloned()
                .collect();
            logs.extend(change.logs);
            agent.conversation.last_seq = current
                .conversation
                .last_seq
                .checked_add(logs.len() as u64)
                .ok_or(SandboxStoreError::SequenceConflict)?;
            AgentConversationMutation {
                settings_fence: None,
                command_claim: None,
                execution_fence: None,
                trigger_execution_fence: None,
                admission_guard: None,
                expected_agent_revision: current.revision,
                expected_last_seq: current.conversation.last_seq,
                runtime_generation: attempted.runtime_generation,
                agent,
                appended_at: at.clone(),
                agent_logs: logs,
            }
        };
        mutation.agent.status = AgentStatus::Failed;
        mutation.agent.failure = Some(failure);
        // Exact Agent CAS fences an in-flight connection/first-input publisher.
        // Its native identity, dispatch evidence, receipts and debt stay intact.
        mutation
            .agent
            .work
            .schedule(AgentWorkStep::None, at.clone());
        let Some(AgentLogKind::StatusChanged { from, to }) = mutation.agent_logs.last_mut() else {
            return Err(SandboxStoreError::Conflict);
        };
        if *from != current.status || *to != AgentStatus::Ready {
            return Err(SandboxStoreError::Conflict);
        }
        *to = AgentStatus::Failed;
        mutation
            .agent
            .validate_conversation()
            .map_err(|_| SandboxStoreError::Conflict)?;
        let mut next = attempted.clone();
        next.active_turn = None;
        next.after_seq = mutation.agent.conversation.last_seq;
        next.assembly = AgentMessageAssembly::default();
        next.outcome.turn_closed = true;
        if !native_terminal {
            // The refused output remains in the native journal. This terminal
            // settles the public task, but does not acknowledge missing content.
            next.output = previous.as_ref().and_then(|value| value.output.clone());
        }
        next.outcome.appended = mutation
            .agent_logs
            .iter()
            .enumerate()
            .map(|(offset, kind)| AgentLogEntry {
                agent_id: current.id.clone(),
                seq: current.conversation.last_seq + offset as u64 + 1,
                appended_at: at.clone(),
                kind: kind.clone(),
            })
            .collect();
        next.validate()?;
        Ok(Self {
            current,
            previous,
            next,
            mutation,
            refused: attempted.outcome.appended,
            original_mutation,
            original_checkpoint,
            compact: !native_terminal,
        })
    }

    /// Only a known-aborted native settlement may select this fallback; an
    /// unknown commit must reconcile its original checkpoint unchanged.
    pub fn compact_failure(&self) -> Result<Option<Self>, SandboxStoreError> {
        if self.compact {
            return Ok(None);
        }
        Self::new(
            self.current.clone(),
            self.previous.clone(),
            self.original_checkpoint.clone(),
            self.original_mutation.clone(),
            false,
        )
        .map(Some)
    }

    /// Preserve the entire original durable prefix outside the active checkpoint
    /// when fixed-size failure records must leave it out of the public log.
    pub fn retained_prefix(&self) -> Option<&AgentMessageCheckpoint> {
        self.previous
            .as_ref()
            .filter(|value| self.compact && !value.assembly.is_empty())
    }

    pub fn current(&self) -> &Agent {
        &self.current
    }
    pub fn previous(&self) -> Option<&AgentMessageCheckpoint> {
        self.previous.as_ref()
    }
    pub fn checkpoint(&self) -> &AgentMessageCheckpoint {
        &self.next
    }
    pub fn mutation(&self) -> &AgentConversationMutation {
        &self.mutation
    }
    pub fn refused(&self) -> &[AgentLogEntry] {
        &self.refused
    }

    pub fn validate_snapshot(
        &self,
        current: &Agent,
        previous: Option<&AgentMessageCheckpoint>,
    ) -> Result<(), SandboxStoreError> {
        if current != &self.current || previous != self.previous.as_ref() {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}
