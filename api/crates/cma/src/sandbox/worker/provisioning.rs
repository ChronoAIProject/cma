use std::future::Future;
use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep,
};
use layer3_adapter::RuntimeProvisionRequest;
use tracing::Instrument;

use super::support::{
    adapter_failure, credential_failure, credential_rotation_at, credential_valid_for_delivery,
    dependency_failure, install_failure, material_matches, operation_id, runtime, WorkerFailure,
};
use super::SandboxWorker;
use crate::sandbox::runtime_mapping::sandbox_spec;

impl SandboxWorker {
    pub(super) fn provision(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> impl Future<Output = Result<(), SandboxStoreError>> + Send + '_ {
        let span = tracing::info_span!("provision", sandbox_id = %sandbox.id);
        // Enter the span only after the provision future is on the heap. The
        // unboxed tracing wrapper leaves a second large frame above retries.
        Box::pin(self.provision_runtime(sandbox, now)).instrument(span)
    }

    async fn provision_runtime(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let now = if self.config.native_create_retry_enabled {
            now.max(SystemTime::now())
        } else {
            now
        };
        let mut timing = super::provision_timing::ProvisionTiming::new();
        if let Some(failure) = self.prepare_sandbox_home(&mut sandbox, now).await? {
            return self
                .handle_provision_failure(sandbox, failure, now, started)
                .await;
        }
        timing.completed("home_preparation");
        if sandbox.work.execution.is_none() {
            let expected = sandbox.revision;
            sandbox.work.execution = Some(self.config.execution);
            sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            self.dependencies
                .store
                .save_sandbox(expected, sandbox.clone())
                .await?;
        }
        timing.completed("execution_policy");
        if sandbox.runtime.is_some() {
            let digest = match sandbox.work.prepared_installation_digest.clone() {
                Some(digest) => Some(digest),
                None => {
                    self.dependencies
                        .store
                        .completed_installation_digest(&sandbox)
                        .await?
                }
            };
            let Some(next) = self
                .adopt_initial_runtime(sandbox, digest.as_deref(), now)
                .await?
            else {
                return Ok(());
            };
            sandbox = next;
        }
        let spec = sandbox_spec(&sandbox, &self.config.deployment_id)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let generation = sandbox.lifecycle.runtime_generation_high_water.max(1);
        let (released, runtime_version) = self
            .initial_runtime_release(&sandbox, &spec, generation)
            .await?;
        let funded_until = match self
            .funding
            .ensure_coverage(&sandbox, SystemTime::now())
            .await
        {
            Ok(coverage) => coverage,
            Err(error) => {
                return self
                    .retry_failure(sandbox, super::support::funding_failure(error), now)
                    .await
            }
        };
        let mut request = RuntimeProvisionRequest::new(generation, spec);
        request.released = released;
        request.funded_until = funded_until;
        layer2_core::sandbox::store::WorkAuthority::current()?;
        match self.dependencies.adapter.provision_runtime(&request).await {
            Ok(handle) => {
                timing.completed("runtime_provision");
                let boot_id = match crate::sandbox::runtime_attachment::provisioned_runtime_status(
                    self.dependencies.adapter.as_ref(),
                    &request.spec,
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
                timing.completed("process_status");
                let mut next = sandbox;
                next.runtime = Some(RuntimeBinding {
                    boot_id,
                    provision_generation: Some(generation),
                    runtime_id: handle.0,
                    runtime_version,
                    bound_at: rfc3339_at(now),
                });
                next.lifecycle.runtime_generation_high_water = generation;
                let result = self
                    .advance_status(
                        next,
                        SandboxStatus::Installing,
                        SandboxWorkStep::RunInstall,
                        now,
                    )
                    .await;
                timing.completed("publish_installing");
                result
            }
            Err(error) => {
                self.handle_provision_failure(sandbox, adapter_failure(error), now, started)
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
        let request = super::initial_runtime::installation_request(&sandbox)
            .expect("execution policy was persisted");
        layer2_core::sandbox::store::WorkAuthority::current()?;
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
        let frame = material.into_runtime_frame(operation_id(&sandbox));
        layer2_core::sandbox::store::WorkAuthority::current()?;
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
            if matches!(
                sandbox.origin,
                layer2_core::sandbox::SandboxOrigin::Lifecycle { .. }
            ) && self
                .dependencies
                .store
                .get_sandbox(&sandbox.id)
                .await?
                .is_some_and(|current| {
                    current.owner == sandbox.owner
                        && current.origin == sandbox.origin
                        && matches!(
                            current.status,
                            SandboxStatus::Retiring | SandboxStatus::Retired
                        )
                })
            {
                // Workspace failure can atomically advance this same parent to retirement.
                return Ok(());
            }
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
}
