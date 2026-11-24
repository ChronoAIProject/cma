use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, RuntimeBinding, Sandbox};
use layer3_adapter::{RestoreBootstrapRequest, RuntimeProvisionRequest};

use super::super::support::{
    adapter_failure, credential_failure, material_matches, runtime, WorkerFailure,
};
use super::super::SandboxWorker;
use super::checkpoint::pinned_checkpoint;
use super::support::{lifecycle_lease, model_error, operation_id, replacement_runtime_version};
use crate::sandbox::runtime_mapping::sandbox_spec;

impl SandboxWorker {
    pub(super) async fn verify_lifecycle_checkpoint(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if sandbox.retained_home_recovery().is_some() {
            return self.verify_lifecycle_home(sandbox, now).await;
        }
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) => checkpoint.clone(),
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_checkpoint_verification(
            &checkpoint,
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }

    pub(super) async fn provision_lifecycle_runtime(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) if sandbox.retained_home_recovery().is_none() => {
                Some(checkpoint.clone())
            }
            _ if sandbox.retained_home_recovery().is_some() => None,
            Ok(_) => unreachable!("archive recovery was matched"),
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let generation = match sandbox.lifecycle.runtime_provision.as_ref() {
            Some(reservation) => reservation.generation,
            None => {
                let agents = self.lifecycle_agents(&sandbox).await?;
                let minimum = match replacement_runtime_generation(&sandbox, &agents) {
                    Ok(generation) => generation,
                    Err(failure) => {
                        return self.handle_lifecycle_failure(sandbox, failure, now).await
                    }
                };
                let stamp = rfc3339_at(now);
                let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
                let mut next = sandbox.clone();
                next.reserve_lifecycle_runtime_generation(
                    minimum,
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                )
                .map_err(model_error)?;
                return self.complete_sandbox_only_phase(sandbox, next, stamp).await;
            }
        };
        if let Some(failure) = self.prepare_sandbox_home(&mut sandbox, now).await? {
            return self.handle_lifecycle_failure(sandbox, failure, now).await;
        }
        if sandbox.retained_home_recovery().is_some() {
            if let Err(failure) = self.verify_retained_home_and_absence(&sandbox).await {
                return self.handle_lifecycle_failure(sandbox, failure, now).await;
            }
        }
        let mut spec = sandbox_spec(&sandbox, &self.config.deployment_id)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        if let Some(target) = sandbox.admitted_runtime_update_target() {
            spec.runtime_image = Some(target.into());
        }
        let funded_until = match self
            .funding
            .ensure_coverage(&sandbox, SystemTime::now())
            .await
        {
            Ok(coverage) => coverage,
            Err(error) => {
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        super::super::support::funding_failure(error),
                        now,
                    )
                    .await
            }
        };
        let mut request = RuntimeProvisionRequest::new(generation, spec);
        request.funded_until = funded_until;
        let retained_transition = sandbox.admitted_retained_runtime_transition();
        let provisioned = if let Some(binding) = sandbox.lifecycle.suspended_runtime.as_deref() {
            match crate::sandbox::runtime_attachment::recovery_runtime_request(
                &sandbox,
                binding,
                &self.config.deployment_id,
            ) {
                Ok(retained) if retained_transition.is_some() => {
                    let transition = retained_transition.expect("retained transition was matched");
                    if !self.lifecycle_state_matches(&sandbox).await? {
                        return Err(SandboxStoreError::Conflict);
                    }
                    let admission = layer3_adapter::RetainedStartupPolicyUpgrade {
                        operation_id: &transition.operation_id,
                        predecessor: &transition.request.predecessor,
                        target_image: &transition.request.target_image,
                        successor_generation: transition.successor_generation,
                        journal_recovery: transition.request.journal_recovery,
                    };
                    request.spec = retained.spec.clone();
                    request.spec.runtime_image = Some(admission.target_image.into());
                    request.spec.retained_journal_recovery = Some(admission.journal_recovery);
                    match self.retained_startup_policy() {
                        Ok(policy) => {
                            policy
                                .upgrade_retained_runtime_policy(&retained, &request, &admission)
                                .await
                        }
                        Err(failure) => {
                            return self.handle_lifecycle_failure(sandbox, failure, now).await
                        }
                    }
                }
                Ok(retained)
                    if sandbox
                        .admitted_runtime_update_target()
                        .is_some_and(|target| target != binding.runtime_version) =>
                {
                    self.dependencies
                        .adapter
                        .upgrade_runtime(&retained, &request)
                        .await
                }
                Ok(retained) => {
                    self.dependencies
                        .adapter
                        .resume_runtime(&retained, &request)
                        .await
                }
                Err(error) => Err(error),
            }
        } else {
            self.dependencies.adapter.provision_runtime(&request).await
        };
        let handle = match provisioned {
            Ok(handle) => handle,
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                    .await
            }
        };
        let native_status = if let Some(checkpoint) = checkpoint {
            let restore = checkpoint.restore_request().map_err(model_error)?;
            let bootstrap = RestoreBootstrapRequest::new(
                operation_id(&sandbox, "bootstrap_restore")?,
                sandbox.id.to_string(),
                restore.clone(),
            );
            let receipt = match self
                .dependencies
                .adapter
                .bootstrap_restore(&handle, &bootstrap)
                .await
            {
                Ok(receipt) => receipt,
                Err(error) => {
                    return self
                        .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                        .await
                }
            };
            if receipt.operation_id != bootstrap.operation_id
                || receipt.request_sha256 != bootstrap.request_sha256
                || receipt.restore_request_sha256 != restore.computed_sha256()
                || !receipt.installation_inherited
            {
                if let Err(error) = self.dependencies.adapter.teardown(&handle).await {
                    tracing::warn!(sandbox_id = %sandbox.id, runtime_id = %handle.0, error = %error, "invalid restore bootstrap left replacement cleanup unconfirmed");
                }
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "runtime returned invalid restore bootstrap evidence",
                        ),
                        now,
                    )
                    .await;
            }
            None
        } else {
            match self
                .verify_native_runtime(&sandbox, &handle, generation, false)
                .await
            {
                Ok(status) => Some(status),
                Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
            }
        };

        let policy = if let (Some(transition), Some(status)) =
            (retained_transition, native_status.as_ref())
        {
            match self
                .verify_retained_policy_readback(&handle, transition, &status.boot_id)
                .await
            {
                Ok(observation) => Some(observation),
                Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
            }
        } else {
            None
        };

        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        // Bind the same process whose generation, home, installation and empty
        // owner state were just validated. A second observation could name a
        // replacement boot whose recovery state was never checked.
        let boot_id = if let Some(status) = native_status {
            Some(status.boot_id)
        } else {
            match crate::sandbox::runtime_attachment::provisioned_runtime_status(
                self.dependencies.adapter.as_ref(),
                &sandbox_spec(&sandbox, &self.config.deployment_id)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
                &handle,
                generation,
            )
            .await
            {
                Ok(status) => status.map(|status| status.boot_id),
                Err(error) => {
                    return self
                        .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                        .await;
                }
            }
        };
        let mut next = sandbox.clone();
        let binding = RuntimeBinding {
            boot_id,
            provision_generation: Some(generation),
            runtime_id: handle.0,
            runtime_version: replacement_runtime_version(
                &sandbox,
                &self.config.default_version(self.config.release().as_ref()),
            ),
            bound_at: stamp.clone(),
        };
        if let Some(observation) = policy {
            next.bind_retained_lifecycle_runtime(
                binding,
                observation,
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
        } else {
            next.bind_lifecycle_runtime(binding, &lease.holder, lease.fence, stamp.clone())
        }
        .map_err(model_error)?;
        let committed = self
            .complete_sandbox_only_phase(sandbox, next.clone(), stamp)
            .await;
        if committed.is_ok() || self.lifecycle_state_matches(&next).await? {
            Ok(())
        } else {
            committed
        }
    }

    pub(super) async fn deliver_lifecycle_credential(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        match self
            .dependencies
            .credentials
            .restore_channel_credential(&sandbox)
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
        if sandbox.needs_service_access_credential() {
            return self.renew_lifecycle_service_access(sandbox, now).await;
        }
        let Some(handle) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("replacement runtime binding is absent"),
                    now,
                )
                .await;
        };
        let Some(authority) = sandbox.credential.current.clone() else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("current credential authority is absent"),
                    now,
                )
                .await;
        };
        let material = match self
            .dependencies
            .credentials
            .material(&sandbox.id, authority.generation)
            .await
        {
            Ok(Some(material)) if material_matches(&sandbox, &authority, &material) => material,
            Ok(_) => {
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "durable current credential does not match its authority",
                        ),
                        now,
                    )
                    .await
            }
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, credential_failure(error), now)
                    .await
            }
        };
        let frame = material.into_runtime_frame(operation_id(&sandbox, "deliver_credential")?);
        if let Err(error) = self
            .dependencies
            .adapter
            .deliver_credential(&handle, frame)
            .await
        {
            return self
                .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                .await;
        }
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_credential_delivery(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }

    pub(super) async fn restore_lifecycle_home(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(handle) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("replacement runtime binding is absent"),
                    now,
                )
                .await;
        };
        if sandbox.retained_home_recovery().is_some() {
            return self.accept_lifecycle_home(sandbox, &handle, now).await;
        }
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) => checkpoint,
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let request = checkpoint.restore_request().map_err(model_error)?;
        if let Err(error) = self.dependencies.adapter.restore(&handle, &request).await {
            return self
                .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                .await;
        }
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_home_restore(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }
}

pub(super) fn replacement_runtime_generation(
    sandbox: &Sandbox,
    agents: &[Agent],
) -> Result<u64, WorkerFailure> {
    let next_agent_generation = agents
        .iter()
        .filter(|agent| agent.status.is_live())
        .filter_map(|agent| agent.conversation.runtime_generation)
        .map(|generation| {
            generation.checked_add(1).ok_or_else(|| {
                WorkerFailure::terminal_validation("agent runtime generation is exhausted")
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or(1);
    let next_authority_generation = sandbox
        .lifecycle
        .operation
        .as_ref()
        .ok_or_else(|| WorkerFailure::terminal_security("lifecycle authority is absent"))?
        .authority()
        .generation
        .checked_add(1)
        .ok_or_else(|| WorkerFailure::terminal_validation("lifecycle authority is exhausted"))?;
    Ok(next_agent_generation.max(next_authority_generation))
}
