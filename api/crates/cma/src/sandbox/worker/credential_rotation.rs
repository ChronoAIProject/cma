use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep};

use super::{support::credential_failure, SandboxWorker};

impl SandboxWorker {
    pub(super) async fn begin_rotation(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if !sandbox.credential.source.is_managed() {
            return self.activate(sandbox, now).await;
        }
        self.advance_status(
            sandbox,
            SandboxStatus::CredentialRotating,
            SandboxWorkStep::MintRotatedCredential,
            now,
        )
        .await
    }

    pub(super) async fn revoke_rotated(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(debt) = sandbox.credential.revoke_debt.first().cloned() else {
            return self
                .advance(
                    sandbox,
                    SandboxWorkStep::FinishCredentialRotation,
                    now,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RevokeRotatedCredential,
                    },
                )
                .await;
        };
        match self
            .dependencies
            .credentials
            .revoke(&sandbox, &debt.provider_key_id)
            .await
        {
            Ok(()) => {
                if let Err(error) = self
                    .dependencies
                    .credentials
                    .forget_provider(&sandbox.id, &debt.provider_key_id)
                    .await
                {
                    return self
                        .retry_failure(sandbox, credential_failure(error), now)
                        .await;
                }
                sandbox.credential.revoke_debt.remove(0);
                let next = if sandbox.credential.revoke_debt.is_empty() {
                    SandboxWorkStep::FinishCredentialRotation
                } else {
                    SandboxWorkStep::RevokeRotatedCredential
                };
                self.advance(
                    sandbox,
                    next,
                    now,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RevokeRotatedCredential,
                    },
                )
                .await
            }
            Err(error) => {
                self.retry_failure(sandbox, credential_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn finish_rotation(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.activate(sandbox, now).await
    }
}
