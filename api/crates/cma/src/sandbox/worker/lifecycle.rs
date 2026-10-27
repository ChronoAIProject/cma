use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxPausePhase,
    SandboxRevivePhase,
};

use super::SandboxWorker;

mod checkpoint;
mod conversations;
mod home_recovery;
mod pause;
mod queue;
mod reopen_output;
mod restore;
mod service_access;
mod support;
mod tombstones;

impl SandboxWorker {
    pub(super) async fn run_lifecycle(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // A restore eventually redelivers a credential even when its service
        // generation floor does not require minting. Gate the whole operation
        // before checkpoint, runtime, Agent, or retained-key effects.
        if matches!(
            sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive { .. })
                | Some(SandboxLifecycleOperation::ImageRotation { .. })
        ) {
            if let Err(error) = self
                .dependencies
                .credentials
                .require_lifecycle_restore(&sandbox)
                .await
            {
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        super::support::credential_failure(error),
                        now,
                    )
                    .await;
            }
        }
        if sandbox.runtime.is_some()
            && !crate::sandbox::runtime_attachment::native_pause_teardown(&sandbox)
        {
            if let Err(error) = crate::sandbox::runtime_attachment::adopt_bound_runtime(
                self.dependencies.adapter.as_ref(),
                &sandbox,
                &self.config.deployment_id,
            )
            .await
            {
                return self
                    .handle_lifecycle_failure(sandbox, super::support::adapter_failure(error), now)
                    .await;
            }
        }
        let operation =
            sandbox.lifecycle.operation.clone().ok_or_else(|| {
                SandboxStoreError::Corrupt("lifecycle operation is absent".into())
            })?;
        match operation {
            SandboxLifecycleOperation::Pause { phase, .. } => match phase {
                SandboxPausePhase::CaptureAndPublishCheckpoint => {
                    self.capture_and_publish(sandbox, now).await
                }
                SandboxPausePhase::PauseAgents => self.pause_lifecycle_agents(sandbox, now).await,
                SandboxPausePhase::TeardownRuntime => {
                    self.teardown_lifecycle_runtime(sandbox, now).await
                }
                SandboxPausePhase::PublishPaused => self.publish_paused(sandbox, now).await,
                SandboxPausePhase::ResumeWriters => self.cancel_pause(sandbox, now).await,
            },
            SandboxLifecycleOperation::Revive { phase, .. } => {
                self.run_restore_phase(sandbox, phase, now).await
            }
            SandboxLifecycleOperation::ImageRotation { phase, .. } => match phase {
                SandboxImageRotationPhase::CaptureAndPublishCheckpoint => {
                    self.capture_and_publish(sandbox, now).await
                }
                SandboxImageRotationPhase::PauseAgents => {
                    self.pause_lifecycle_agents(sandbox, now).await
                }
                SandboxImageRotationPhase::TeardownRuntime => {
                    self.teardown_lifecycle_runtime(sandbox, now).await
                }
                SandboxImageRotationPhase::CheckpointVerification => {
                    self.verify_lifecycle_checkpoint(sandbox, now).await
                }
                SandboxImageRotationPhase::RuntimeProvisioning => {
                    self.provision_lifecycle_runtime(sandbox, now).await
                }
                SandboxImageRotationPhase::CredentialDelivery => {
                    self.deliver_lifecycle_credential(sandbox, now).await
                }
                SandboxImageRotationPhase::HomeRestore => {
                    self.restore_lifecycle_home(sandbox, now).await
                }
                SandboxImageRotationPhase::ConversationReopen => {
                    self.reopen_lifecycle_conversations(sandbox, now).await
                }
                SandboxImageRotationPhase::QueuedMessageDelivery => {
                    self.deliver_lifecycle_queue(sandbox, now).await
                }
                SandboxImageRotationPhase::PublishActive => {
                    self.publish_restored_active(sandbox, now).await
                }
            },
        }
    }

    async fn run_restore_phase(
        &self,
        sandbox: Sandbox,
        phase: SandboxRevivePhase,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        match phase {
            SandboxRevivePhase::CheckpointVerification => {
                self.verify_lifecycle_checkpoint(sandbox, now).await
            }
            SandboxRevivePhase::RuntimeProvisioning => {
                self.provision_lifecycle_runtime(sandbox, now).await
            }
            SandboxRevivePhase::CredentialDelivery => {
                self.deliver_lifecycle_credential(sandbox, now).await
            }
            SandboxRevivePhase::HomeRestore => self.restore_lifecycle_home(sandbox, now).await,
            SandboxRevivePhase::ConversationReopen => {
                self.reopen_lifecycle_conversations(sandbox, now).await
            }
            SandboxRevivePhase::QueuedMessageDelivery => {
                self.deliver_lifecycle_queue(sandbox, now).await
            }
            SandboxRevivePhase::PublishActive => self.publish_restored_active(sandbox, now).await,
        }
    }
}
