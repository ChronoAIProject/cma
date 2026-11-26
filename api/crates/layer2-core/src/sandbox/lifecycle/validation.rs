use super::*;

impl SandboxLifecycleState {
    pub(super) fn validate_operation(
        &self,
        sandbox: &Sandbox,
        operation: &SandboxLifecycleOperation,
    ) -> Result<(), SandboxModelError> {
        let authority = operation.authority();
        let expected_id = format!(
            "sandbox-lifecycle-v1:{}:{}",
            sandbox.id.as_str(),
            authority.generation
        );
        if operation.expected_status() != sandbox.status
            || authority.generation == 0
            || authority.generation >= self.next_authority
            || authority.operation_id != expected_id
            || authority.operation_id.len() > MAX_LIFECYCLE_OPERATION_ID_BYTES
            || seconds_from_rfc3339(&authority.requested_at).is_none()
        {
            return Err(invalid_lifecycle());
        }
        if operation.checkpoint_epoch().is_some_and(|epoch| {
            self.checkpoints
                .latest
                .as_ref()
                .is_none_or(|checkpoint| checkpoint.epoch != epoch)
        }) {
            return Err(invalid_lifecycle());
        }
        match operation {
            SandboxLifecycleOperation::Pause { .. } => {
                validate_writer_token(authority)?;
            }
            SandboxLifecycleOperation::Revive {
                checkpoint_epoch, ..
            } => {
                if checkpoint_epoch.is_none() && self.home_recovery.is_none() {
                    return Err(invalid_lifecycle());
                }
                if authority.writer_resume_token.is_some()
                    || authority.writer_resume_generation != 0
                {
                    return Err(invalid_lifecycle());
                }
            }
            SandboxLifecycleOperation::ImageRotation {
                target_runtime_version,
                identity,
                ..
            } => {
                validate_writer_token(authority)?;
                if target_runtime_version.trim().is_empty()
                    || target_runtime_version.len() > MAX_RUNTIME_VERSION_BYTES
                    || identity.sandbox_id != sandbox.id
                    || identity.owner != sandbox.owner
                    || identity.profile_id != sandbox.profile.id
                    || identity.profile_revision != sandbox.profile.revision
                    || identity.profile_digest != sandbox.profile.digest
                    || identity.validate().is_err()
                {
                    return Err(invalid_lifecycle());
                }
            }
        }
        Ok(())
    }
}

fn validate_writer_token(authority: &SandboxLifecycleAuthority) -> Result<(), SandboxModelError> {
    let expected = authority.expected_writer_resume_token();
    if authority.writer_resume_token.as_deref() != Some(expected.as_str()) {
        return Err(invalid_lifecycle());
    }
    Ok(())
}
