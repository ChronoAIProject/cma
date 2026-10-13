use super::*;
use layer2_core::sandbox::{Sandbox, SandboxLifecycleOperation, SandboxWorkStep};

impl AgentAccessWorker {
    pub(super) async fn paused(
        &self,
        record: &AgentAccessRecord,
        sandbox: &Sandbox,
    ) -> Result<(), AgentAccessError> {
        let expected = record
            .progress
            .pause_operation_id
            .as_deref()
            .ok_or(AgentAccessError::Invalid)?;
        require_latest_operation(sandbox, expected)?;
        if let Some(operation) = &sandbox.lifecycle.operation {
            return if matches!(operation, SandboxLifecycleOperation::Pause { .. })
                && operation.authority().operation_id == expected
            {
                Ok(())
            } else {
                Err(AgentAccessError::Invalid)
            };
        }
        if sandbox.status != SandboxStatus::Paused {
            return Err(AgentAccessError::Invalid);
        }
        if sandbox.runtime.is_none() && settled(sandbox) {
            self.advance(record, Phase::Granting).await?;
        }
        Ok(())
    }

    pub(super) async fn grant(
        &self,
        record: &AgentAccessRecord,
        sandbox: &Sandbox,
    ) -> Result<(), AgentAccessError> {
        require_latest_operation(
            sandbox,
            record
                .progress
                .pause_operation_id
                .as_deref()
                .ok_or(AgentAccessError::Invalid)?,
        )?;
        let approved = record.approved_services()?;
        if sandbox.status != SandboxStatus::Paused
            || sandbox
                .credential
                .effective_reaches()
                .iter()
                .any(|service| !approved.contains(service))
        {
            return Err(AgentAccessError::Invalid);
        }
        if !self.connections_ready(record).await? {
            return Ok(());
        }
        // Grant, parent event, generation floor and phase share the same lease
        // transaction. A timed-out worker cannot grant after a later decision.
        self.store
            .grant_access_request_services(record.lease.as_ref().ok_or(AgentAccessError::Conflict)?)
            .await?;
        Ok(())
    }

    pub(super) async fn revive(
        &self,
        record: &AgentAccessRecord,
        sandbox: &Sandbox,
    ) -> Result<(), AgentAccessError> {
        let expected = record
            .progress
            .revive_operation_id
            .as_ref()
            .or(record.progress.pause_operation_id.as_ref())
            .ok_or(AgentAccessError::Invalid)?;
        require_latest_operation(sandbox, expected)?;
        if !self.connections_ready(record).await? {
            return Ok(());
        }
        if record.progress.revive_operation_id.is_none() {
            // Persist the exact revival operation with its workflow lease before
            // the ordinary lifecycle worker performs any runtime effects.
            self.store
                .schedule_access_request_revival(
                    record.lease.as_ref().ok_or(AgentAccessError::Conflict)?,
                )
                .await?;
            return Ok(());
        }
        if let Some(operation) = &sandbox.lifecycle.operation {
            return if matches!(operation, SandboxLifecycleOperation::Revive { .. })
                && Some(&operation.authority().operation_id)
                    == record.progress.revive_operation_id.as_ref()
            {
                Ok(())
            } else {
                Err(AgentAccessError::Invalid)
            };
        }
        if sandbox.status == SandboxStatus::CredentialRotating {
            return Ok(());
        }
        if sandbox.status != SandboxStatus::Active {
            return Err(AgentAccessError::Invalid);
        }
        if !settled(sandbox) {
            return Ok(());
        }
        let floor = record
            .progress
            .grant_generation_floor
            .ok_or(AgentAccessError::Invalid)?;
        let Some(current) = &sandbox.credential.current else {
            return Ok(());
        };
        let Some(runtime) = &sandbox.runtime else {
            return Ok(());
        };
        if !sandbox.credential.delivered
            || current.generation <= floor
            || !record
                .approved_services()?
                .iter()
                .all(|service| sandbox.credential.effective_reaches().contains(service))
        {
            return Ok(());
        }
        let mut progress = record.progress.clone();
        progress.delivered_credential = Some(current.clone());
        progress.delivered_runtime = Some(runtime.clone());
        self.store
            .advance_access_request(
                record.lease.as_ref().ok_or(AgentAccessError::Conflict)?,
                Phase::Reviving,
                Phase::Notifying,
                progress,
            )
            .await?;
        Ok(())
    }
}

fn require_latest_operation(sandbox: &Sandbox, expected: &str) -> Result<(), AgentAccessError> {
    let latest = format!(
        "sandbox-lifecycle-v1:{}:{}",
        sandbox.id,
        sandbox.lifecycle.next_authority.saturating_sub(1)
    );
    if latest != expected {
        return Err(AgentAccessError::Invalid);
    }
    Ok(())
}

pub(super) fn settled(sandbox: &Sandbox) -> bool {
    (sandbox.work.step == SandboxWorkStep::None
        || (sandbox.status == SandboxStatus::Active
            && sandbox.work.step == SandboxWorkStep::WaitForCredentialRotation))
        && sandbox.work.lease.is_none()
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && sandbox.credential.pending.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::agent_access_fixtures;

    #[test]
    fn a_newer_completed_lifecycle_cannot_leave_old_access_work_applying() {
        let mut sandbox = agent_access_fixtures::sandbox("superseded");
        sandbox.lifecycle.next_authority = 3;
        let expected = format!("sandbox-lifecycle-v1:{}:2", sandbox.id);
        assert!(require_latest_operation(&sandbox, &expected).is_ok());
        // Same resulting status and no active operation still represent a
        // different owner decision when the lifecycle generation advanced.
        for status in [SandboxStatus::Active, SandboxStatus::Paused] {
            sandbox.status = status;
            sandbox.lifecycle.next_authority = 4;
            assert_eq!(
                require_latest_operation(&sandbox, &expected),
                Err(AgentAccessError::Invalid)
            );
        }
    }
}
