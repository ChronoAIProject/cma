use std::future::Future;
use std::pin::Pin;
use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxPausePhase,
    SandboxRevivePhase,
};

use super::SandboxWorker;

mod checkpoint;
mod checkpoint_retry;
mod conversations;
mod home_recovery;
mod pause;
mod publish_active;
mod queue;
pub(in crate::sandbox) mod reopen_output;
mod reopen_owned;
mod reopen_refusal;
pub(super) use reopen_owned::{PendingReopen, ReopenGates};
mod restore;
mod service_access;
mod support;
mod tombstones;
mod unpublished_recovery;

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
        self.execute_lifecycle_phase(sandbox, operation, now).await
    }

    // Construct only the selected phase future outside the polling frame.
    fn execute_lifecycle_phase(
        &self,
        sandbox: Sandbox,
        operation: SandboxLifecycleOperation,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<(), SandboxStoreError>> + Send + '_>> {
        match operation {
            SandboxLifecycleOperation::Pause { phase, .. } => match phase {
                SandboxPausePhase::CaptureAndPublishCheckpoint => {
                    Box::pin(self.capture_and_publish(sandbox, now))
                }
                SandboxPausePhase::PauseAgents => {
                    Box::pin(self.pause_lifecycle_agents(sandbox, now))
                }
                SandboxPausePhase::TeardownRuntime => {
                    Box::pin(self.teardown_lifecycle_runtime(sandbox, now))
                }
                SandboxPausePhase::PublishPaused => Box::pin(self.publish_paused(sandbox, now)),
                SandboxPausePhase::ResumeWriters => Box::pin(self.cancel_pause(sandbox, now)),
            },
            SandboxLifecycleOperation::Revive { phase, .. } => {
                self.run_restore_phase(sandbox, phase, now)
            }
            SandboxLifecycleOperation::ImageRotation { phase, .. } => match phase {
                SandboxImageRotationPhase::CaptureAndPublishCheckpoint => {
                    Box::pin(self.capture_and_publish(sandbox, now))
                }
                SandboxImageRotationPhase::PauseAgents => {
                    Box::pin(self.pause_lifecycle_agents(sandbox, now))
                }
                SandboxImageRotationPhase::TeardownRuntime => {
                    Box::pin(self.teardown_lifecycle_runtime(sandbox, now))
                }
                SandboxImageRotationPhase::CheckpointVerification => {
                    Box::pin(self.verify_lifecycle_checkpoint(sandbox, now))
                }
                SandboxImageRotationPhase::RuntimeProvisioning => {
                    Box::pin(self.provision_lifecycle_runtime(sandbox, now))
                }
                SandboxImageRotationPhase::CredentialDelivery => {
                    Box::pin(self.deliver_lifecycle_credential(sandbox, now))
                }
                SandboxImageRotationPhase::HomeRestore => {
                    Box::pin(self.restore_lifecycle_home(sandbox, now))
                }
                SandboxImageRotationPhase::ConversationReopen => {
                    Box::pin(self.reopen_lifecycle_conversations(sandbox, now))
                }
                SandboxImageRotationPhase::QueuedMessageDelivery => {
                    Box::pin(self.deliver_lifecycle_queue(sandbox, now))
                }
                SandboxImageRotationPhase::PublishActive => {
                    Box::pin(self.publish_restored_active(sandbox, now))
                }
            },
        }
    }

    fn run_restore_phase(
        &self,
        sandbox: Sandbox,
        phase: SandboxRevivePhase,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<(), SandboxStoreError>> + Send + '_>> {
        match phase {
            SandboxRevivePhase::CheckpointVerification => {
                Box::pin(self.verify_lifecycle_checkpoint(sandbox, now))
            }
            SandboxRevivePhase::RuntimeProvisioning => {
                Box::pin(self.provision_lifecycle_runtime(sandbox, now))
            }
            SandboxRevivePhase::CredentialDelivery => {
                Box::pin(self.deliver_lifecycle_credential(sandbox, now))
            }
            SandboxRevivePhase::HomeRestore => Box::pin(self.restore_lifecycle_home(sandbox, now)),
            SandboxRevivePhase::ConversationReopen => {
                Box::pin(self.reopen_lifecycle_conversations(sandbox, now))
            }
            SandboxRevivePhase::QueuedMessageDelivery => {
                Box::pin(self.deliver_lifecycle_queue(sandbox, now))
            }
            SandboxRevivePhase::PublishActive => {
                Box::pin(self.publish_restored_active(sandbox, now))
            }
        }
    }
}
