use super::lifecycle_execution_support::invalid_execution;
use super::{
    Sandbox, SandboxCredentialAuthority, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxModelError, SandboxRevivePhase,
};

impl Sandbox {
    pub fn needs_service_access_credential(&self) -> bool {
        let floor = self.credential.service_access_generation_floor();
        floor > 0
            && self
                .credential
                .current
                .as_ref()
                .is_none_or(|current| current.generation <= floor)
    }

    /// Publish replacement authority before delivering it to a revived runtime.
    pub fn replace_lifecycle_service_access_credential(
        &mut self,
        authority: SandboxCredentialAuthority,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.needs_service_access_credential()
            || self.credential.pending.is_some()
            || self.credential.next_generation() != Some(authority.generation)
            || !matches!(
                self.lifecycle.operation,
                Some(SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::CredentialDelivery,
                    ..
                }) | Some(SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::CredentialDelivery,
                    ..
                })
            )
        {
            return Err(invalid_execution());
        }
        self.apply_claimed(holder, fence, at.clone(), at.clone(), |next| {
            if let Some(previous) = next.credential.current.take() {
                if previous.provider_key_id == authority.provider_key_id {
                    return Err(invalid_execution());
                }
                next.credential.queue_revoke(previous.provider_key_id, at);
            }
            next.credential.key_generation = authority.generation;
            next.credential.current = Some(authority);
            next.credential.delivered = true;
            Ok(())
        })
    }
}
