//! Durable lifecycle admission before the runtime exists. This authorizes no runtime effect.
use super::{
    Agent, AgentOrigin, AgentStatus, AgentWorkStep, Sandbox, SandboxOrigin, SandboxStatus,
    Workspace, WorkspaceOrigin, WorkspaceStatus,
};

impl Sandbox {
    pub fn accepts_lifecycle_bootstrap(&self, operation_id: &str) -> bool {
        matches!(&self.origin, SandboxOrigin::Lifecycle { operation_id: original } if original == operation_id)
            && matches!(
                self.status,
                SandboxStatus::Provisioning | SandboxStatus::Installing
            )
            && self.lifecycle.operation.is_none()
            && self.lifecycle.checkpoint_capture.is_none()
            && self.lifecycle.suspended_runtime.is_none()
            && self.retired_at.is_none()
            && self.failure.is_none()
    }
}

/// The exact profile child is reserved once; its normal worker still prepares it later.
pub(super) fn bootstrap_workspace(sandbox: &Sandbox, workspace: &Workspace) -> bool {
    workspace.owner == sandbox.owner
        && workspace.sandbox_id == sandbox.id
        && matches!(workspace.origin, WorkspaceOrigin::SandboxProfile { .. })
        && workspace.origin_matches_sandbox_profile(sandbox)
        && matches!(
            workspace.status,
            WorkspaceStatus::Preparing | WorkspaceStatus::Ready
        )
        && workspace.failed.is_none()
        && workspace.removed_at.is_none()
}

impl Agent {
    /// An exact lifecycle parent, never a generic Preparing/Installing permission.
    pub fn bootstrap_operation<'a>(&'a self, parent: &Sandbox) -> Option<&'a str> {
        let AgentOrigin::Lifecycle { operation_id } = &self.origin else {
            return None;
        };
        (self.owner == parent.owner
            && self.sandbox_id == parent.id
            && self.status == AgentStatus::Starting
            && self.work.step == AgentWorkStep::WaitForWorkspace
            && self.requested_stop_reason.is_none()
            && self.has_no_runtime_effects()
            && parent.accepts_lifecycle_bootstrap(operation_id))
        .then_some(operation_id)
    }

    pub fn bootstrap_lineage<'a>(
        &'a self,
        parent: &Sandbox,
        workspace: &Workspace,
    ) -> Option<&'a str> {
        self.bootstrap_operation(parent)
            .filter(|_| self.workspace_id == workspace.id && bootstrap_workspace(parent, workspace))
    }

    /// Every runtime mutation records debt before dispatch. Absence of both debt
    /// and receipts on this lifecycle reservation proves there is nothing to remove.
    pub fn has_no_runtime_effects(&self) -> bool {
        matches!(self.origin, AgentOrigin::Lifecycle { .. })
            && self.work.receipts.is_empty()
            && self.work.cleanup_debt.is_empty()
            && self.work.resolved_skills.is_none()
            && self.work.warm_conversation_id.is_none()
            && self.work.live_skills.is_empty()
            && self.conversation.conversation_id.is_none()
            && self.conversation.runtime_generation.is_none()
            && self.conversation.active_turn.is_none()
            && self.conversation.operation.is_none()
            && self.conversation.queued_messages.is_empty()
            && self.ready_at.is_none()
    }
}
