//! Durable service deadlines are independent of runtime completion receipts.
use serde::{Deserialize, Serialize};

use super::{Agent, AgentStatus, TypedFailure};
use crate::ids::{AgentCommandId, AgentId};
mod settlement;
pub use settlement::TurnTimeoutSettlement;
pub mod approval;

#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod tests;

/// Immutable execution identity captured before the corresponding dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnTimeoutBinding {
    pub agent_id: AgentId,
    pub creation_key: String,
    pub creation_fingerprint: String,
    pub command_id: AgentCommandId,
    pub turn_sequence: u64,
    pub operation_sequence: u64,
    pub runtime_generation: u64,
    pub runtime_boot: String,
    pub conversation_id: String,
    pub prompt_id: String,
}

impl TurnTimeoutBinding {
    pub fn validate(&self) -> Result<(), TurnTimeoutError> {
        let prompt = layer3_adapter::PromptIdentity::new(
            &self.conversation_id,
            self.runtime_generation,
            self.turn_sequence,
        );
        if !identifier(self.agent_id.as_str(), 256)
            || !identifier(self.command_id.as_str(), 256)
            || !identifier(&self.creation_key, 256)
            || !hex(&self.creation_fingerprint, 64)
            || !hex(&self.runtime_boot, 32)
            || !identifier(&self.conversation_id, 1024)
            || self.runtime_generation == 0
            || self.turn_sequence == 0
            || self.operation_sequence == 0
            || self.prompt_id != prompt.as_str()
        {
            return Err(TurnTimeoutError::Invalid);
        }
        Ok(())
    }

    pub fn matches_incarnation(&self, agent: &Agent) -> bool {
        self.agent_id == agent.id
            && self.creation_key == agent.creation_key
            && self.creation_fingerprint == agent.creation_fingerprint
    }

    pub fn matches_active_turn(&self, agent: &Agent) -> bool {
        self.matches_incarnation(agent)
            && agent.conversation.active_turn == Some(self.turn_sequence)
            && agent.conversation.runtime_generation == Some(self.runtime_generation)
            && agent.conversation.conversation_id.as_deref() == Some(&self.conversation_id)
            && matches!(
                agent.status,
                AgentStatus::Working | AgentStatus::WaitingApproval
            )
            && agent.requested_stop_reason.is_none()
    }
}

/// A policy class does not assert that an external operation was never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnTimeoutCause {
    ServiceDeadline,
    ApprovalBudget,
    ApprovalDenied,
    ApprovalExpired,
    ApprovalDisconnected,
}

impl TurnTimeoutCause {
    pub fn failure(self) -> TypedFailure {
        let (class, detail) = match self {
            Self::ServiceDeadline => (
                "agent_turn_timeout",
                "The server ended this turn because its time limit was reached. Earlier work may have completed. Review the conversation before sending a new message; this turn will not be replayed automatically.",
            ),
            Self::ApprovalBudget => (
                "approval_timeout",
                "The turn ended because its confirmed approval wait exceeded the allowed time. Earlier work may have completed. Execution cleanup is still required before continuing.",
            ),
            Self::ApprovalDenied => ("approval_denied", "The required approval was denied. This turn has ended and will not be replayed automatically."),
            Self::ApprovalExpired => ("approval_expired", "The required approval expired. This turn has ended and will not be replayed automatically."),
            Self::ApprovalDisconnected => ("approval_wait_disconnected", "The approval wait ended without a resumable operation. This turn has ended and will not be replayed automatically."),
        };
        TypedFailure {
            class: class.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutEffectState {
    Unknown,
    Dispatched,
    NotSent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum TimeoutDisposition {
    Dormant,
    Armed,
    Terminal {
        committed_at_ms: i64,
        effect: TimeoutEffectState,
    },
    Superseded,
}

/// Stored with the turn before any worker attempts terminal publication.
/// Disabling admission does not remove this obligation or its reuse fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnTimeout {
    version: u32,
    binding: TurnTimeoutBinding,
    action_id: String,
    cause: TurnTimeoutCause,
    revision: u64,
    waiting_since_ms: i64,
    deadline_at_ms: i64,
    observed_at_ms: i64,
    disposition: TimeoutDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval: Option<approval::ApprovalBudget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TurnTimeoutError {
    #[error("invalid turn timeout state")]
    Invalid,
    #[error("turn timeout authority changed")]
    Conflict,
    #[error("turn timeout deadline has not elapsed")]
    NotDue,
}

impl TurnTimeout {
    /// The caller chooses a deadline under a separately admitted policy. Silence
    /// alone cannot authorize the ApprovalBudget cause.
    pub fn admit(
        binding: TurnTimeoutBinding,
        action_id: String,
        cause: TurnTimeoutCause,
        waiting_since_ms: i64,
        deadline_at_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        let value = Self {
            version: 1,
            binding,
            action_id,
            cause,
            revision: 1,
            waiting_since_ms,
            deadline_at_ms,
            observed_at_ms: waiting_since_ms,
            disposition: TimeoutDisposition::Armed,
            approval: None,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), TurnTimeoutError> {
        self.binding.validate()?;
        if let Some(approval) = &self.approval {
            approval.validate()?;
            if self.cause == TurnTimeoutCause::ServiceDeadline
                || (self.cause == TurnTimeoutCause::ApprovalBudget
                    && (approval
                        .waiting_since_ms()
                        .is_some_and(|start| start != self.waiting_since_ms)
                        || approval
                            .deadline_at_ms()
                            .is_some_and(|deadline| deadline != self.deadline_at_ms)
                        || (self.is_open()
                            && self.is_armed() != approval.deadline_at_ms().is_some())))
            {
                return Err(TurnTimeoutError::Invalid);
            }
        }
        if self.version != 1
            || uuid::Uuid::parse_str(&self.action_id).is_err()
            || self.revision == 0
            || self.revision > (1_u64 << 53) - 1
            || self.waiting_since_ms < 0
            || self.deadline_at_ms < self.waiting_since_ms
            || self.observed_at_ms < self.waiting_since_ms
            || (self.cause != TurnTimeoutCause::ServiceDeadline && self.approval.is_none())
            || (self.approval.is_none() && matches!(self.disposition, TimeoutDisposition::Dormant))
            || matches!(self.disposition, TimeoutDisposition::Terminal { committed_at_ms, .. }
                if committed_at_ms < self.deadline_at_ms || committed_at_ms != self.observed_at_ms)
        {
            return Err(TurnTimeoutError::Invalid);
        }
        Ok(())
    }

    pub fn binding(&self) -> &TurnTimeoutBinding {
        &self.binding
    }
    pub fn action_id(&self) -> &str {
        &self.action_id
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cause(&self) -> TurnTimeoutCause {
        self.cause
    }
    pub fn waiting_since_ms(&self) -> i64 {
        self.waiting_since_ms
    }
    pub fn deadline_at_ms(&self) -> i64 {
        self.deadline_at_ms
    }
    pub fn is_armed(&self) -> bool {
        matches!(self.disposition, TimeoutDisposition::Armed)
    }
    pub fn is_open(&self) -> bool {
        matches!(
            self.disposition,
            TimeoutDisposition::Armed | TimeoutDisposition::Dormant
        )
    }
    pub fn blocks_dispatch(&self) -> bool {
        matches!(self.disposition, TimeoutDisposition::Terminal { .. })
            || (matches!(self.disposition, TimeoutDisposition::Superseded)
                && self
                    .approval
                    .as_ref()
                    .is_some_and(approval::ApprovalBudget::has_operations))
    }

    /// This transition is committed together with the turn terminal. It changes
    /// no remote effect knowledge and creates no runtime acknowledgement.
    pub fn expire(&self, expected_revision: u64, now_ms: i64) -> Result<Self, TurnTimeoutError> {
        self.require_revision(expected_revision)?;
        if !self.is_armed() {
            return Err(TurnTimeoutError::Conflict);
        }
        let now_ms = now_ms.max(self.observed_at_ms);
        if now_ms < self.deadline_at_ms {
            return Err(TurnTimeoutError::NotDue);
        }
        let mut next = self.clone();
        next.revision += 1;
        next.observed_at_ms = now_ms;
        next.disposition = TimeoutDisposition::Terminal {
            committed_at_ms: now_ms,
            effect: TimeoutEffectState::Unknown,
        };
        next.validate()?;
        Ok(next)
    }

    /// Only a winning original-turn result/control can disarm the deadline.
    /// A terminal policy decision cannot be superseded by a late result.
    pub fn supersede(&self, expected_revision: u64) -> Result<Self, TurnTimeoutError> {
        self.require_revision(expected_revision)?;
        if !self.is_open() {
            return Err(TurnTimeoutError::Conflict);
        }
        let mut next = self.clone();
        next.revision += 1;
        next.disposition = TimeoutDisposition::Superseded;
        next.validate()?;
        Ok(next)
    }

    fn require_revision(&self, revision: u64) -> Result<(), TurnTimeoutError> {
        self.validate()?;
        if revision != self.revision {
            return Err(TurnTimeoutError::Conflict);
        }
        Ok(())
    }

    pub fn admit_approval(
        binding: TurnTimeoutBinding,
        action_id: String,
        limits: approval::ApprovalLimits,
        clock: approval::ApprovalClockBounds,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        let mut value = Self::admit(
            binding,
            action_id,
            TurnTimeoutCause::ServiceDeadline,
            now_ms,
            now_ms,
        )?;
        value.cause = TurnTimeoutCause::ApprovalBudget;
        value.approval = Some(approval::ApprovalBudget::admit(limits, clock, now_ms)?);
        value.disposition = TimeoutDisposition::Dormant;
        value.validate()?;
        Ok(value)
    }

    pub fn register_approval_operation(
        &self,
        expected_revision: u64,
        binding: approval::ApprovalOperationBinding,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        self.require_revision(expected_revision)?;
        if !self.is_open() || self.cause != TurnTimeoutCause::ApprovalBudget {
            return Err(TurnTimeoutError::Conflict);
        }
        let approval = self.approval.as_ref().ok_or(TurnTimeoutError::Conflict)?;
        self.with_approval(approval.register(binding, now_ms)?, now_ms)
    }

    pub fn observe_pending_approval(
        &self,
        expected_revision: u64,
        receipt: &approval::PendingApprovalReceipt,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        self.require_revision(expected_revision)?;
        if !self.is_open() || self.cause != TurnTimeoutCause::ApprovalBudget {
            return Err(TurnTimeoutError::Conflict);
        }
        let approval = self.approval.as_ref().ok_or(TurnTimeoutError::Conflict)?;
        self.with_approval(approval.pending(receipt, now_ms)?, now_ms)
    }

    pub fn observe_approval_resolution(
        &self,
        expected_revision: u64,
        receipt: &approval::ResolvedApprovalReceipt,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        self.require_revision(expected_revision)?;
        if !self.is_open() || self.cause != TurnTimeoutCause::ApprovalBudget {
            return Err(TurnTimeoutError::Conflict);
        }
        let approval = self.approval.as_ref().ok_or(TurnTimeoutError::Conflict)?;
        let resolved = approval.resolve(receipt, now_ms)?;
        let mut next = self.with_approval(resolved, now_ms)?;
        if receipt.resolution != approval::ApprovalResolution::Approved {
            next.cause = match receipt.resolution {
                approval::ApprovalResolution::Denied => TurnTimeoutCause::ApprovalDenied,
                approval::ApprovalResolution::Expired => TurnTimeoutCause::ApprovalExpired,
                approval::ApprovalResolution::Disconnected => {
                    TurnTimeoutCause::ApprovalDisconnected
                }
                approval::ApprovalResolution::Approved => unreachable!(),
            };
            next.deadline_at_ms = next.observed_at_ms;
            next.disposition = TimeoutDisposition::Armed;
        }
        next.validate()?;
        Ok(next)
    }

    fn with_approval(
        &self,
        approval: approval::ApprovalBudget,
        now_ms: i64,
    ) -> Result<Self, TurnTimeoutError> {
        let now_ms = now_ms.max(self.observed_at_ms);
        if self.approval.as_ref() == Some(&approval) && now_ms == self.observed_at_ms {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.revision += 1;
        next.observed_at_ms = now_ms;
        if let Some(start) = approval.waiting_since_ms() {
            next.waiting_since_ms = start;
        }
        if let Some(deadline) = approval.deadline_at_ms() {
            next.deadline_at_ms = deadline;
            next.disposition = TimeoutDisposition::Armed;
        } else {
            next.disposition = TimeoutDisposition::Dormant;
        }
        next.approval = Some(approval);
        next.validate()?;
        Ok(next)
    }
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Ordinary output can close its original turn, but cannot admit or settle a
/// policy without the command/lane transaction that owns that decision.
pub fn validate_ordinary_publication(
    current: &Agent,
    mutation: &super::store::AgentConversationMutation,
) -> Result<(), super::store::SandboxStoreError> {
    use super::{store::SandboxStoreError, AgentLogKind};
    let external: Vec<_> = mutation
        .agent_logs
        .iter()
        .filter(|kind| {
            matches!(
                kind,
                AgentLogKind::ExternalApprovalRequested { .. }
                    | AgentLogKind::ExternalApprovalResolved { .. }
            )
        })
        .cloned()
        .collect();
    if current.conversation.turn_timeout == mutation.agent.conversation.turn_timeout {
        return if external.is_empty() {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        };
    }
    let Some(previous) = &current.conversation.turn_timeout else {
        return Err(SandboxStoreError::Conflict);
    };
    let closing = mutation.agent_logs.iter().any(|kind| match kind {
        AgentLogKind::TurnCompleted { turn_sequence }
        | AgentLogKind::TurnInterrupted { turn_sequence }
        | AgentLogKind::TurnFailed { turn_sequence, .. } => {
            *turn_sequence == previous.binding().turn_sequence
        }
        _ => false,
    });
    if closing
        && current.conversation.active_turn == Some(previous.binding().turn_sequence)
        && mutation.agent.conversation.active_turn.is_none()
        && previous.supersede(previous.revision()).ok().as_ref()
            == mutation.agent.conversation.turn_timeout.as_ref()
    {
        let next = mutation
            .agent
            .conversation
            .turn_timeout
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if next
            .approval_events_since(previous)
            .map_err(|_| SandboxStoreError::Conflict)?
            == external
        {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        }
    } else {
        Err(SandboxStoreError::Conflict)
    }
}
