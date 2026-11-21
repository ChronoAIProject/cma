use super::{BindingState, BindingWork, ProviderBinding};
use crate::sandbox::{Sandbox, SandboxCredentialAuthority, SandboxStatus, SandboxWorkStep};
use crate::CoreError;

impl ProviderBinding {
    pub fn rotate_credential(&self, hash: String, at: String) -> Result<Self, CoreError> {
        if self.state != BindingState::Ready
            || self.work != BindingWork::Idle
            || self.previous.is_some()
            || self.key_sha256 == hash
        {
            return Err(conflict());
        }
        let mut next = self.clone();
        next.key_sha256 = hash;
        if self.existing_agent.is_none() {
            next.state = BindingState::Provisioning;
            next.work = BindingWork::DeliverCredential;
        }
        next.advance_revision(at).map_err(CoreError::InvalidState)?;
        Ok(next)
    }

    /// Retain the conversation generation and hierarchy while the existing
    /// credential worker delivers a new custody generation to the live runtime.
    pub fn admit_credential_delivery(
        &self,
        sandbox: &Sandbox,
        authority: SandboxCredentialAuthority,
    ) -> Result<Sandbox, CoreError> {
        if self.work != BindingWork::DeliverCredential
            || self.state != BindingState::Provisioning
            || sandbox.id != self.sandbox_id
            || sandbox.owner != self.owner
            || sandbox.credential.source != self.credential_source()
            || !matches!(
                sandbox.status,
                SandboxStatus::Active | SandboxStatus::Paused
            )
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.work.lease.is_some()
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.background_checkpoint.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.credential.pending.is_some()
            || sandbox.credential.current.is_none()
            || !sandbox.credential.revoke_debt.is_empty()
            || sandbox.credential.next_generation() != Some(authority.generation)
            || authority.provider_key_id != format!("provided-{}", self.key_sha256)
            || !sandbox.credential.source.accepts(&authority)
            || crate::profiles::content_digest(&sandbox.credential.effective_reaches())
                .ok()
                .as_ref()
                != Some(&authority.grant_fingerprint)
        {
            return Err(conflict());
        }
        let mut next = sandbox.clone();
        next.credential.key_generation = authority.generation;
        if sandbox.status == SandboxStatus::Paused {
            // Revival reads the current encrypted generation before admitting work.
            next.credential.current = Some(authority);
        } else {
            next.credential.pending = Some(authority);
            next.status = SandboxStatus::CredentialRotating;
            next.work.schedule(
                SandboxWorkStep::DeliverRotatedCredential,
                self.updated_at.clone(),
            );
        }
        next.revision = next.revision.checked_add(1).ok_or_else(conflict)?;
        next.updated_at = self.updated_at.clone();
        next.validate().map_err(|_| conflict())?;
        Ok(next)
    }

    pub fn credential_delivery_complete(&self, sandbox: &Sandbox) -> bool {
        sandbox.id == self.sandbox_id
            && sandbox.owner == self.owner
            && matches!(
                sandbox.status,
                SandboxStatus::Active | SandboxStatus::Paused
            )
            && sandbox.credential.source == self.credential_source()
            && sandbox.credential.pending.is_none()
            && sandbox.credential.current.as_ref().is_some_and(|current| {
                current.provider_key_id == format!("provided-{}", self.key_sha256)
            })
    }
}

fn conflict() -> CoreError {
    CoreError::InvalidState("binding credential rotation is not currently admissible".into())
}
