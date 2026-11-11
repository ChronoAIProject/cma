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
}

impl RecoveryActivation {
    /// Domain check only. The store must separately prove the exact immutable
    /// completed result witness against this current Agent and fresh authority.
    pub fn capture(
        context: &CapacityRecoveryContext<'_>,
        work: &RecoveryWork,
    ) -> Result<Self, RecoveryError> {
        work.validate()?;
        let a = context.agent;
        let s = context.sandbox;
        let w = context.workspace;
        if work.phase != RecoveryPhase::Completed
            || !context.same_lineage()
            || RecoveryTarget::from_context(context)? != work.target
            || a.work.next_fence != work.next_fence
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
        {
            return Err(RecoveryError::Conflict);
        }
        Ok(Self {
            completed: work.clone(),
            agent: a.clone(),
        })
    }

    pub fn completed(&self) -> &RecoveryWork {
        &self.completed
    }
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
    /// The committed claim's native fence, never a newly invented lease.
    pub fn writer_fence(&self) -> u64 {
        self.completed.next_fence - 1
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
            || confirmation.target != self.completed.target
            || confirmation.authority_fence != self.writer_fence()
            || status != self.agent.status
            || matches!(confirmation.native_state, ConfirmedNativeState::Idle)
                != self.completed.target.active_turn.is_none()
        {
            return Err(RecoveryError::InvalidEvidence);
        }
        Ok(())
    }
}
