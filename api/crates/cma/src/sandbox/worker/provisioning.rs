use std::collections::BTreeMap;
use std::time::SystemTime;

use layer2_core::profiles::ModelAccess;
use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep,
};
use layer3_adapter::{
    CredentialFrame, InstallRequest, RuntimeCredential, SandboxCapacity, SandboxSpec,
};

use super::support::{
    adapter_failure, credential_failure, credential_rotation_at, credential_valid_for_delivery,
    dependency_failure, install_failure, material_matches, operation_id, runtime, WorkerFailure,
};
use super::SandboxWorker;

impl SandboxWorker {
    fn sandbox_spec(&self, sandbox: &Sandbox) -> SandboxSpec {
        let environment = sandbox
            .profile
            .content
            .environment_variables
            .iter()
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<BTreeMap<_, _>>();
        SandboxSpec {
            sandbox_id: sandbox.id.0.clone(),
            capacity: SandboxCapacity {
                cpu_millis: sandbox.profile.content.cpu_millis,
                memory_mib: sandbox.profile.content.memory_mib,
            },
            environment,
            reaches: sandbox
                .credential
                .reaches
                .iter()
                .map(ToString::to_string)
                .collect(),
            own_credential: sandbox.profile.content.model_access == ModelAccess::OwnCredentials,
            created_by: Some(self.config.deployment_id.clone()),
            ..SandboxSpec::default()
        }
    }

    pub(super) async fn provision(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let spec = self.sandbox_spec(&sandbox);
        match self.dependencies.adapter.provision(&spec).await {
            Ok(handle) => {
                let mut next = sandbox;
                next.runtime = Some(RuntimeBinding {
                    runtime_id: handle.0,
                    runtime_version: self.config.runtime_version.clone(),
                    bound_at: rfc3339_at(now),
                });
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
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(runtime) = runtime(&sandbox) else {
            return self.fail_corrupt_claim(sandbox, now).await;
        };
        let attached = match self
            .dependencies
            .adapter
            .provision(&self.sandbox_spec(&sandbox))
            .await
        {
            Ok(handle) if handle == runtime => handle,
            Ok(_) => {
                return self
                    .handle_failure(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "provider reattached a different runtime identity",
                        ),
                        now,
                    )
                    .await
            }
            Err(error) => {
                return self
                    .handle_failure(sandbox, adapter_failure(error), now)
                    .await
            }
        };
        let request = InstallRequest::new(
            format!("sandbox-install-v1:{}", sandbox.id),
            sandbox.profile.content.software_install_commands.clone(),
        );
        match self
            .dependencies
            .adapter
            .run_install(&attached, &request)
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
        let committed = self
            .advance(
                next,
                next_step,
                now,
                SandboxEventKind::CredentialChanged {
                    key_generation: generation,
                },
            )
            .await;
        if committed.is_ok() {
            if let Err(error) = self
                .dependencies
                .credentials
                .forget(&material.sandbox_id, generation)
                .await
            {
                tracing::warn!(sandbox_id = %material.sandbox_id, generation, error = %error, "delivered credential custody cleanup deferred");
            }
        }
        committed
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
            .revoke(&sandbox.owner, &debt.provider_key_id)
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
}
