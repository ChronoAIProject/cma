use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{SandboxLifecycleAgentScope, SandboxStoreError};
use layer2_core::sandbox::{
    AgentStatus, Sandbox, SandboxEventKind, SandboxLifecycleOperation, SandboxStatus,
};

use super::super::support::{credential_failure, credential_rotation_at, WorkerFailure};
use super::super::SandboxWorker;
use super::checkpoint::require_quiet_agent;
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    pub(super) async fn publish_restored_active(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Some(transition) = sandbox.admitted_retained_runtime_transition() {
            let binding = sandbox.runtime.as_ref().ok_or_else(|| {
                SandboxStoreError::Corrupt("admitted runtime binding is missing".into())
            })?;
            let boot = binding.boot_id.as_deref().ok_or_else(|| {
                SandboxStoreError::Corrupt("admitted runtime boot is missing".into())
            })?;
            let handle = layer3_adapter::RuntimeHandle::new(&binding.runtime_id);
            if let Err(failure) = self
                .verify_retained_policy_readback(&handle, transition, boot)
                .await
            {
                return self.handle_lifecycle_failure(sandbox, failure, now).await;
            }
            if !self.lifecycle_state_matches(&sandbox).await? {
                return Err(SandboxStoreError::Conflict);
            }
        }
        match self
            .dependencies
            .credentials
            .publish_channel_active(&sandbox)
            .await
        {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, credential_failure(error), now)
                    .await
            }
        }
        let agents = self.lifecycle_agents(&sandbox).await?;
        if !agents
            .iter()
            .filter(|agent| sandbox.restores_agent(agent))
            .all(|agent| {
                agent.status == AgentStatus::Ready
                    && require_quiet_agent(agent).is_ok()
                    && agent.conversation.queued_messages.is_empty()
            })
        {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::waiting("restored agents are not settled"),
                    now,
                )
                .await;
        }
        let rotation_at = if sandbox.credential.source.is_managed() {
            credential_rotation_at(&sandbox)
        } else {
            Some(now)
        };
        let Some(rotation_at) = rotation_at else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("current credential rotation time is invalid"),
                    now,
                )
                .await;
        };
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let operation = sandbox.lifecycle.operation.clone();
        let mut next = sandbox.clone();
        next.publish_lifecycle_active(
            rfc3339_at(rotation_at),
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        let mut events = vec![SandboxEventKind::StatusChanged {
            from: sandbox.status,
            to: next.status,
        }];
        match operation.filter(|_| next.status == SandboxStatus::Active) {
            Some(SandboxLifecycleOperation::Revive { .. }) => {
                events.push(SandboxEventKind::Revived)
            }
            Some(SandboxLifecycleOperation::ImageRotation {
                target_runtime_version,
                ..
            }) => events.push(SandboxEventKind::Rotated {
                runtime_version: target_runtime_version,
            }),
            _ => {}
        }
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            stamp,
            events,
        )
        .await
    }
}
