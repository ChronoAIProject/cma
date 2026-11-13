use super::*;
use layer2_core::sandbox::SandboxEventKind;
use layer3_adapter::{CredentialFrame, RuntimeCredential};

use super::support::{
    adapter_failure, credential_failure, credential_valid_for_delivery, material_matches,
    operation_id, WorkerFailure,
};

impl SandboxWorker {
    pub(super) async fn deliver(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        // Custody lookup is read-only and adds at most one request per already
        // admitted worker. Keep it call-local: inspection refusal drops the
        // lookup and any fetched bytes, while a lookup failure waits for the
        // original inspection result. Nothing is delivered before both finish.
        let lookup = async {
            let material = match sandbox.credential.pending.as_ref() {
                Some(pending) => {
                    self.dependencies
                        .credentials
                        .material(&sandbox.id, pending.generation)
                        .await
                }
                None => Ok(None),
            };
            Ok::<_, layer3_adapter::AdapterError>(material)
        };
        let (runtime, material) = match tokio::try_join!(self.inspect_for_work(&sandbox), lookup) {
            Ok(value) => value,
            Err(error) => {
                return self
                    .handle_failure(sandbox, adapter_failure(error), now + started.elapsed())
                    .await
            }
        };
        let now = now + started.elapsed();
        let Some(pending) = sandbox.credential.pending.clone() else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        let material = match material {
            Ok(Some(material)) if material_matches(&sandbox, &pending, &material) => material,
            Ok(_) => {
                return self
                    .handle_failure(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "durable credential value does not match pending authority",
                        ),
                        now,
                    )
                    .await
            }
            Err(error) => {
                return self
                    .handle_failure(sandbox, credential_failure(error), now)
                    .await
            }
        };
        if !credential_valid_for_delivery(&sandbox, now) {
            return self
                .handle_failure(
                    sandbox,
                    WorkerFailure::terminal_security(
                        "pending credential reached its delivery expiry margin",
                    ),
                    now,
                )
                .await;
        }
        let frame = CredentialFrame {
            operation_id: operation_id(&sandbox),
            value: RuntimeCredential::new(material.value.expose()),
            custody_generation: Some(pending.generation),
        };
        if let Err(error) = self
            .dependencies
            .adapter
            .deliver_credential(&runtime, frame)
            .await
        {
            return self
                .handle_failure(sandbox, adapter_failure(error), now)
                .await;
        }
        let generation = pending.generation;
        let mut next = sandbox;
        let rotated = next.work.step == SandboxWorkStep::DeliverRotatedCredential;
        if let Some(current) = next.credential.current.take() {
            next.credential
                .queue_revoke(current.provider_key_id, rfc3339_at(now));
        }
        next.credential.current = next.credential.pending.take();
        next.credential.delivered = true;
        let next_step = if rotated {
            if next.credential.revoke_debt.is_empty() {
                SandboxWorkStep::FinishCredentialRotation
            } else {
                SandboxWorkStep::RevokeRotatedCredential
            }
        } else if next.profile.content.workspace_profiles.is_empty() {
            SandboxWorkStep::Activate
        } else {
            SandboxWorkStep::CreateProfileWorkspace(0)
        };
        self.advance(
            next,
            next_step,
            now,
            SandboxEventKind::CredentialChanged {
                key_generation: generation,
            },
        )
        .await
    }
}
