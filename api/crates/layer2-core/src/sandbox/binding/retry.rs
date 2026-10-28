use super::{BindingState, BindingWork, ProvidedCredential, ProviderBinding};
use crate::sandbox::{
    Sandbox, SandboxStatus, SandboxWorkStep, Workspace, WorkspaceStatus, WorkspaceWorkStep,
};
use crate::CoreError;

impl ProviderBinding {
    /// A manager's identical PUT can acknowledge recovery of the original
    /// hierarchy. The transaction adapter must prove current encrypted custody
    /// and fence these exact records before committing the returned binding.
    pub fn retry_recovered_hierarchy(
        &self,
        sandbox: &Sandbox,
        workspace: &Workspace,
        credential: &ProvidedCredential,
        at: String,
    ) -> Result<Self, CoreError> {
        self.validate().map_err(CoreError::InvalidState)?;
        let current = sandbox.credential.current.as_ref().ok_or_else(conflict)?;
        if !matches!(self.state, BindingState::Failed { .. })
            || self.work != BindingWork::Idle
            || self.previous.is_some()
            || self.existing_agent.is_some()
            || self.profile.environment.is_none()
            || credential.sha256() != self.key_sha256
            || sandbox.id != self.sandbox_id
            || sandbox.owner != self.owner
            || sandbox.status != SandboxStatus::Active
            || sandbox.retired_at.is_some()
            || sandbox.failure.is_some()
            || sandbox.runtime.is_none()
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.work.lease.is_some()
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.background_checkpoint.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.lifecycle.home_recovery.is_some()
            || sandbox.lifecycle.unpublished_recovery.is_some()
            || sandbox.lifecycle.runtime_provision.is_some()
            || sandbox.lifecycle.suspended_runtime.is_some()
            || sandbox
                .runtime_update_request()
                .is_some_and(|request| request.completed_at.is_none())
            || sandbox.credential.source != self.credential_source()
            || !sandbox.credential.validate()
            || sandbox.credential.pending.is_some()
            || !sandbox.credential.revoke_debt.is_empty()
            || current.generation != sandbox.credential.key_generation
            || current.provider_key_id != format!("provided-{}", self.key_sha256)
            || crate::profiles::content_digest(&sandbox.credential.effective_reaches())
                .ok()
                .as_ref()
                != Some(&current.grant_fingerprint)
            || workspace.id != self.workspace_id
            || workspace.sandbox_id != self.sandbox_id
            || workspace.owner != self.owner
            || workspace.status != WorkspaceStatus::Ready
            || workspace.work.step != WorkspaceWorkStep::None
            || workspace.work.lease.is_some()
            || workspace.failed.is_some()
            || workspace.removed_at.is_some()
        {
            return Err(conflict());
        }
        let mut next = self.clone();
        next.state = BindingState::Ready;
        next.advance_revision(at).map_err(CoreError::InvalidState)?;
        Ok(next)
    }
}

fn conflict() -> CoreError {
    CoreError::InvalidState("binding's original hierarchy has not completed recovery".into())
}
