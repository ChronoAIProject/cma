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

    pub(super) async fn grant(&self, record: &AgentAccessRecord) -> Result<(), AgentAccessError> {
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

pub(super) fn settled(sandbox: &Sandbox) -> bool {
    (sandbox.work.step == SandboxWorkStep::None
        || (sandbox.status == SandboxStatus::Active
            && sandbox.work.step == SandboxWorkStep::WaitForCredentialRotation))
        && sandbox.work.lease.is_none()
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && sandbox.credential.pending.is_none()
}
