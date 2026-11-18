use super::*;

/// Observation identity survives terminal failure without retaining execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxFailedLifecycleAdmission {
    pub operation_id: String,
    pub requested_at: String,
}

impl SandboxLifecycleState {
    pub(crate) fn fail_operation(&mut self) -> Result<(), SandboxModelError> {
        let authority = self
            .operation
            .as_ref()
            .ok_or_else(invalid_lifecycle)?
            .authority();
        let admission = SandboxFailedLifecycleAdmission {
            operation_id: authority.operation_id.clone(),
            requested_at: authority.requested_at.clone(),
        };
        self.cancel_operation();
        self.failed_operation = Some(admission);
        Ok(())
    }

    pub(super) fn validate_failed_operation(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), SandboxModelError> {
        let Some(admission) = &self.failed_operation else {
            return Ok(());
        };
        let generation = self
            .next_authority
            .checked_sub(1)
            .filter(|generation| *generation > 0)
            .ok_or_else(invalid_lifecycle)?;
        if sandbox.status != SandboxStatus::Failed
            || self.operation.is_some()
            || admission.operation_id != format!("sandbox-lifecycle-v1:{}:{generation}", sandbox.id)
            || seconds_from_rfc3339(&admission.requested_at).is_none()
        {
            return Err(invalid_lifecycle());
        }
        Ok(())
    }
}
