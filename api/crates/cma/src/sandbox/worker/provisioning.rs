use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep,
};
use layer3_adapter::{
    CredentialFrame, InstallRequest, RuntimeAdoptionRequest, RuntimeCredential, RuntimeHandle,
    RuntimeProvisionRequest,
};

use super::support::{
    adapter_failure, credential_failure, credential_rotation_at, credential_valid_for_delivery,
    dependency_failure, install_failure, material_matches, operation_id, runtime, WorkerFailure,
};
use super::SandboxWorker;
use crate::sandbox::runtime_mapping::sandbox_spec;

impl SandboxWorker {
    pub(super) async fn provision(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Some(failure) = self.prepare_sandbox_home(&mut sandbox, now).await? {
            return self.handle_failure(sandbox, failure, now).await;
        }
        let spec = sandbox_spec(&sandbox, &self.config.deployment_id);
        if sandbox.work.execution.is_none() {
            let expected = sandbox.revision;
            sandbox.work.execution = Some(self.config.execution);
            sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            self.dependencies
                .store
                .save_sandbox(expected, sandbox.clone())
                .await?;
        }
        // A warm admission already owns an installed runtime. Prove that exact
        // binding before replaying its installation receipt and minting a key.
        if let Some(binding) = sandbox.runtime.clone() {
            let adoption = RuntimeAdoptionRequest::new(
                binding.provision_generation.unwrap_or(1),
                RuntimeHandle::new(&binding.runtime_id),
                spec.clone(),
            )
            .with_expected_image(&binding.runtime_version);
            match self.dependencies.adapter.adopt_runtime(&adoption).await {
                Ok(Some(_)) => {
                    return self
                        .advance_status(
                            sandbox,
                            SandboxStatus::Installing,
                            SandboxWorkStep::RunInstall,
                            now,
                        )
                        .await
                }
                Ok(None) => {
                    // No user data or credential has been delivered at this
                    // stage. Persist a new generation before replacing a lost
                    // warm runtime; retries reuse this reservation.
                    let expected = sandbox.revision;
                    sandbox.runtime = None;
                    sandbox.lifecycle.runtime_generation_high_water = sandbox
                        .lifecycle
                        .runtime_generation_high_water
                        .checked_add(1)
                        .ok_or(SandboxStoreError::Conflict)?;
                    sandbox.revision =
                        expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
                    self.dependencies
                        .store
                        .save_sandbox(expected, sandbox.clone())
                        .await?;
                }
                Err(error) => {
                    return self
                        .handle_failure(sandbox, adapter_failure(error), now)
                        .await
                }
            }
        }
        let generation = sandbox.lifecycle.runtime_generation_high_water.max(1);
        match self
            .dependencies
            .adapter
            .provision_runtime(&RuntimeProvisionRequest::new(generation, spec))
            .await
        {
            Ok(handle) => {
                let boot_id = match crate::sandbox::runtime_attachment::provisioned_runtime_status(
                    self.dependencies.adapter.as_ref(),
                    &sandbox_spec(&sandbox, &self.config.deployment_id),
                    &handle,
                    generation,
                )
                .await
                {
                    Ok(status) => status.map(|status| status.boot_id),
                    Err(error) => {
                        return self
                            .handle_failure(sandbox, adapter_failure(error), now)
                            .await;
                    }
                };
                let mut next = sandbox;
                next.runtime = Some(RuntimeBinding {
                    boot_id,
                    provision_generation: Some(generation),
                    runtime_id: handle.0,
                    runtime_version: next
                        .profile
                        .content
                        .runtime_image
                        .clone()
                        .unwrap_or_else(|| self.config.runtime_version.clone()),
                    bound_at: rfc3339_at(now),
                });
                next.lifecycle.runtime_generation_high_water = generation;
                self.advance_status(
                    next,
                    SandboxStatus::Installing,
                    SandboxWorkStep::RunInstall,
                    now,
                )
                .await
            }
            Err(error) => {
                self.handle_failure(sandbox, adapter_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn install(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(runtime) = runtime(&sandbox) else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        if sandbox.work.execution.is_none() {
            let expected = sandbox.revision;
            sandbox.work.execution = Some(self.config.execution);
            sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            self.dependencies
                .store
                .save_sandbox(expected, sandbox.clone())
                .await?;
        }
        let execution = sandbox
            .work
            .execution
            .expect("execution policy was persisted");
        let request = InstallRequest::with_timeouts(
            format!("sandbox-install-v1:{}", sandbox.id),
            sandbox.profile.content.software_install_commands.clone(),
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        );
        match self
            .dependencies
            .adapter
            .run_install(&runtime, &request)
            .await
        {
            Ok(_) => {
                self.advance(
                    sandbox,
                    SandboxWorkStep::MintInitialCredential,
                    now,
                    SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RunInstall,
                    },
                )
                .await
            }
            Err(error) => {
                self.handle_failure(sandbox, install_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn mint(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(generation) = sandbox.credential.next_generation() else {
            return self
                .handle_failure(
                    sandbox,
                    WorkerFailure::terminal_validation("credential generation exhausted"),
                    now,
                )
                .await;
        };
        match self
            .dependencies
            .credentials
            .issue(&sandbox, generation, now)
            .await
        {
            Ok(material) => {
                let completed = sandbox.work.step;
                let mut next = sandbox;
                next.credential.key_generation = generation;
                next.credential.pending = Some(material.authority);
                if !credential_valid_for_delivery(&next, now) {
                    return self
                        .handle_failure(
                            next,
                            WorkerFailure::terminal_security(
                                "identity service issued a credential inside the expiry margin",
                            ),
                            now,
                        )
                        .await;
                }
                let next_step = match completed {
                    SandboxWorkStep::MintInitialCredential => {
                        SandboxWorkStep::DeliverInitialCredential
                    }
                    SandboxWorkStep::MintRotatedCredential => {
                        SandboxWorkStep::DeliverRotatedCredential
                    }
                    _ => unreachable!("mint is called only for mint steps"),
                };
                self.advance(
                    next,
                    next_step,
                    now,
                    SandboxEventKind::WorkStepCompleted { step: completed },
                )
                .await
            }
            Err(error) => {
                self.handle_failure(sandbox, credential_failure(error), now)
                    .await
            }
        }
    }

    pub(super) async fn deliver(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(runtime) = runtime(&sandbox) else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        let Some(pending) = sandbox.credential.pending.clone() else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        let material = match self
            .dependencies
            .credentials
            .material(&sandbox.id, pending.generation)
            .await
        {
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

    pub(super) async fn create_workspace(
        &self,
        sandbox: Sandbox,
        index: u16,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(runtime) = runtime(&sandbox) else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        let Some(profile) = sandbox
            .profile
            .content
            .workspace_profiles
            .get(usize::from(index))
        else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        if let Err(error) = self
            .dependencies
            .workspaces
            .create_profile_workspace(&sandbox, &runtime, profile, index, &operation_id(&sandbox))
            .await
        {
            return self
                .handle_failure(sandbox, dependency_failure(error), now)
                .await;
        }
        let following = usize::from(index) + 1;
        let next = if following < sandbox.profile.content.workspace_profiles.len() {
            SandboxWorkStep::CreateProfileWorkspace(
                u16::try_from(following).expect("profile workspace count is bounded by 16"),
            )
        } else {
            SandboxWorkStep::Activate
        };
        self.advance(
            sandbox,
            next,
            now,
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::CreateProfileWorkspace(index),
            },
        )
        .await
    }

    pub(super) async fn activate(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if !sandbox.credential.source.is_managed() {
            return self
                .advance_status(sandbox, SandboxStatus::Active, SandboxWorkStep::None, now)
                .await;
        }
        let Some(ready) = credential_rotation_at(&sandbox) else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        self.advance_status_at(
            sandbox,
            SandboxStatus::Active,
            SandboxWorkStep::WaitForCredentialRotation,
            now,
            ready,
        )
        .await
    }

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
