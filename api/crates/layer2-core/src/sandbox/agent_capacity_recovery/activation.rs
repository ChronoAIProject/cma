//! Resumable attachment to an already committed completion. This is not a new
//! admission, lease or publication, and must never cause input replay.
use super::{ports::RecoveryError, state::*, CapacityRecoveryContext};
use crate::sandbox::{
    Agent, AgentStatus, AgentWorkStep, SandboxStatus, SandboxWorkStep, WorkspaceStatus,
    WorkspaceWorkStep, WorktreeStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryActivation {
    completed: RecoveryWork,
    agent: Agent,
    target: RecoveryTarget,
}

impl RecoveryActivation {
    /// Domain check only. The store must separately prove the exact immutable
    /// completed result witness against this current Agent and fresh authority.
    pub fn capture(
        context: &CapacityRecoveryContext<'_>,
        work: &RecoveryWork,
    ) -> Result<Self, RecoveryError> {
        if RecoveryTarget::from_context(context)? != work.target
            || context.agent.work.next_fence != work.next_fence
        {
            return Err(RecoveryError::Conflict);
        }
        Self::capture_current(context, work)
    }

    /// Domain proposal for a positively proven current publication. The store
    /// must verify the original receipt and current full-Agent publication in
    /// the same transaction as fresh authority, and recapture after native IO.
    /// This does not rewrite the completed work or authorize input replay.
    pub fn capture_current(
        context: &CapacityRecoveryContext<'_>,
        work: &RecoveryWork,
    ) -> Result<Self, RecoveryError> {
        work.validate()?;
        let a = context.agent;
        let s = context.sandbox;
        let w = context.workspace;
        let target = RecoveryTarget::from_context(context)?;
        if work.phase != RecoveryPhase::Completed
            || !context.same_lineage()
            || !same_native_identity(&target, &work.target)
            || a.work.next_fence < work.next_fence
            || work.next_fence <= 1
            || a.work.step != AgentWorkStep::None
            || a.work.lease.is_some()
            || a.failure.is_some()
            || a.requested_stop_reason.is_some()
            || a.stop_reason.is_some()
            || a.stopped_at.is_some()
            || a.deleted_at.is_some()
            || a.work.cleanup_not_before.is_some()
            || !matches!(
                a.status,
                AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
            )
            || (a.status == AgentStatus::Ready) != target.active_turn.is_none()
            || target
                .active_turn
                .is_some_and(|seq| seq > a.conversation.last_seq)
            || s.status != SandboxStatus::Active
            || s.retired_at.is_some()
            || !s.credential.delivered
            || s.work.lease.is_some()
            || !matches!(
                s.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || s.lifecycle.operation.is_some()
            || s.lifecycle.checkpoint_capture.is_some()
            || s.lifecycle.runtime_provision.is_some()
            || s.lifecycle.home_recovery.is_some()
            || s.lifecycle.unpublished_recovery.is_some()
            || s.lifecycle.failed_provision.is_some()
            || w.status != WorkspaceStatus::Ready
            || w.removed_at.is_some()
            || w.work.step != WorkspaceWorkStep::None
            || w.work.lease.is_some()
            || context.worktree.status != WorktreeStatus::Ready
            || a.validate_prerequisites(context.worktree).is_err()
            || context.worktree.validate_prerequisites(w).is_err()
            || w.validate_prerequisites(s).is_err()
            || w.resolved_commit.as_ref() != Some(&context.worktree.base_commit)
            || a.validate_conversation().is_err()
        {
            return Err(RecoveryError::Conflict);
        }
        Ok(Self {
            completed: work.clone(),
            agent: a.clone(),
            target,
        })
    }

    pub fn completed(&self) -> &RecoveryWork {
        &self.completed
    }
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
    /// The current target is separate from the immutable recovery result.
    pub fn target(&self) -> &RecoveryTarget {
        &self.target
    }
    /// The currently durable allocated fence, never a newly invented lease.
    pub fn writer_fence(&self) -> u64 {
        self.agent.work.next_fence - 1
    }

    pub fn require_confirmation(
        &self,
        confirmation: &RecoveryConfirmation,
    ) -> Result<(), RecoveryError> {
        let status = match confirmation.native_state {
            ConfirmedNativeState::Idle => AgentStatus::Ready,
            ConfirmedNativeState::Working => AgentStatus::Working,
            ConfirmedNativeState::WaitingApproval => AgentStatus::WaitingApproval,
        };
        if confirmation.operation_key != self.completed.request.key()?
            || confirmation.target != self.target
            || confirmation.authority_fence != self.writer_fence()
            || status != self.agent.status
            || matches!(confirmation.native_state, ConfirmedNativeState::Idle)
                != self.target.active_turn.is_none()
        {
            return Err(RecoveryError::InvalidEvidence);
        }
        Ok(())
    }
}

fn same_native_identity(current: &RecoveryTarget, original: &RecoveryTarget) -> bool {
    current.agent_id == original.agent_id
        && current.creation_key == original.creation_key
        && current.creation_fingerprint == original.creation_fingerprint
        && current.conversation_id == original.conversation_id
        && current.conversation_generation == original.conversation_generation
        && current.runtime == original.runtime
        && current.home_identity == original.home_identity
        && current.original_open == original.original_open
}
