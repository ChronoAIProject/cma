use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{Sandbox, SandboxStoreError};
use layer3_adapter::{
    RetainedStartupPolicyControl, RuntimeHandle, RuntimeStatus, RuntimeStatusRequest,
};

use super::super::support::{adapter_failure, WorkerFailure};
use super::super::SandboxWorker;
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    pub(super) fn retained_startup_policy(
        &self,
    ) -> Result<&dyn RetainedStartupPolicyControl, WorkerFailure> {
        self.dependencies
            .adapter
            .retained_startup_policy()
            .ok_or_else(|| {
                WorkerFailure::retryable("retained runtime startup policy is unavailable")
            })
    }

    pub(super) async fn verify_lifecycle_home(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if sandbox
            .lifecycle
            .unpublished_recovery
            .as_ref()
            .is_some_and(|state| state.quiescence.is_none())
        {
            return self.quiesce_unpublished_runtime(sandbox, now).await;
        }
        if let Err(failure) = self.verify_retained_home_and_absence(&sandbox).await {
            return self.handle_lifecycle_failure(sandbox, failure, now).await;
        }
        let source = sandbox.retained_home_recovery().ok_or_else(|| {
            SandboxStoreError::Corrupt("retained home recovery source is absent".into())
        })?;
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_home_verification(
            source,
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }

    pub(super) async fn verify_retained_home_and_absence(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), WorkerFailure> {
        sandbox
            .require_unpublished_runtime_quiescence()
            .map_err(|_| {
                WorkerFailure::retryable("original runtime quiescence has not been recorded")
            })?;
        let source = sandbox
            .retained_home_recovery()
            .ok_or_else(missing_source)?;
        source.validate_for(sandbox).map_err(|_| missing_source())?;
        let attachment = sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or_else(missing_source)?;
        if attachment.request.policy.is_native_vct()
            && sandbox.lifecycle.suspended_runtime.is_none()
        {
            return Err(WorkerFailure::terminal_security(
                "retained home recovery has no original runtime authority; storage was preserved",
            ));
        }
        let volumes = self
            .dependencies
            .adapter
            .home_volumes()
            .ok_or_else(missing_source)?;
        volumes.verify(&attachment).await.map_err(adapter_failure)?;
        if !self
            .lifecycle_state_matches(sandbox)
            .await
            .map_err(|_| WorkerFailure::retryable("home recovery authority is unavailable"))?
        {
            return Err(WorkerFailure::retryable(
                "home recovery authority was superseded",
            ));
        }
        let old = |observation: &layer3_adapter::RuntimeObservation| {
            observation.sandbox_id == sandbox.id.as_str()
                && observation.generation <= source.source_generation
                && sandbox
                    .lifecycle
                    .suspended_runtime
                    .as_ref()
                    .is_none_or(|binding| binding.runtime_id != observation.handle.0)
        };
        let retained = sandbox.admitted_retained_runtime_transition();
        for observation in self
            .dependencies
            .adapter
            .runtime_inventory()
            .await
            .map_err(adapter_failure)?
            .iter()
            .filter(|observation| old(observation))
        {
            if retained.is_some() {
                return Err(WorkerFailure::retryable(
                    "another runtime exists for the admitted retained home",
                ));
            }
            // Older recovery records can lack a retained binding. Their original
            // storage-owning object must never become process cleanup.
            if observation.handle.as_str().starts_with("agent-sandbox:") {
                return Err(WorkerFailure::terminal_security(
                    "retained runtime authority is unavailable; storage was preserved",
                ));
            }
            self.dependencies
                .adapter
                .reclaim_runtime(observation)
                .await
                .map_err(adapter_failure)?;
        }
        if self
            .dependencies
            .adapter
            .runtime_inventory()
            .await
            .map_err(adapter_failure)?
            .iter()
            .any(old)
        {
            return Err(WorkerFailure::retryable(
                "previous runtime removal is still in progress",
            ));
        }
        if sandbox.lifecycle.runtime_provision.is_none() {
            if let Some(binding) = sandbox.lifecycle.suspended_runtime.as_deref() {
                let request = crate::sandbox::runtime_attachment::recovery_runtime_request(
                    sandbox,
                    binding,
                    &self.config.deployment_id,
                )
                .map_err(adapter_failure)?;
                if let Some(transition) = retained {
                    // The grant already required a stopped predecessor. A new
                    // suspension could change provider state after that proof expired.
                    let predecessor = self
                        .retained_startup_policy()?
                        .inspect_retained_startup_policy(&request)
                        .await
                        .map_err(|_| {
                            WorkerFailure::retryable(
                                "the admitted stopped predecessor could not be verified",
                            )
                        })?;
                    if predecessor != transition.request.predecessor {
                        return Err(WorkerFailure::retryable(
                            "the admitted stopped predecessor has changed",
                        ));
                    }
                    return volumes.verify(&attachment).await.map_err(adapter_failure);
                }
                if sandbox
                    .lifecycle
                    .unpublished_recovery
                    .as_ref()
                    .is_some_and(|state| state.proof.boot_id.is_none())
                {
                    // A completed suspension can lose its reply. Its exact
                    // retained provider graph proves replay without a vanished Pod.
                    if self
                        .dependencies
                        .adapter
                        .verify_suspended_runtime(&request)
                        .await
                        .is_ok()
                    {
                        return volumes.verify(&attachment).await.map_err(adapter_failure);
                    }
                    // Read the full Agent set and current authority, then re-read
                    // the exact stopped container immediately before suspension.
                    let source =
                        self.nonexecuting_recovery_request(sandbox)
                            .await
                            .map_err(|_| {
                                WorkerFailure::retryable(
                                    "original nonexecuting runtime evidence changed; home retained",
                                )
                            })?;
                    self.dependencies
                        .adapter
                        .runtime_recovery()
                        .ok_or_else(missing_source)?
                        .suspend_nonexecuting_recovery(&source)
                        .await
                        .map_err(|_| {
                            WorkerFailure::retryable(
                                "original nonexecuting runtime is not yet verified; home retained",
                            )
                        })?;
                    return volumes.verify(&attachment).await.map_err(adapter_failure);
                }
                self.dependencies
                    .adapter
                    // A lost process does not imply a suspended provider object.
                    // Repeating suspension is safe after a lost acknowledgement.
                    .suspend_runtime(&request)
                    .await
                    .map_err(adapter_failure)?;
            }
        }
        volumes.verify(&attachment).await.map_err(adapter_failure)
    }

    pub(super) async fn verify_native_runtime(
        &self,
        sandbox: &Sandbox,
        runtime: &RuntimeHandle,
        generation: u64,
        credentialed: bool,
    ) -> Result<RuntimeStatus, WorkerFailure> {
        let source = sandbox
            .retained_home_recovery()
            .ok_or_else(missing_source)?;
        source.validate_for(sandbox).map_err(|_| missing_source())?;
        if generation <= source.source_generation {
            return Err(WorkerFailure::terminal_security(
                "replacement runtime generation did not advance",
            ));
        }
        let status = self
            .dependencies
            .adapter
            .runtime_status(runtime)
            .await
            .map_err(adapter_failure)?;
        status
            .validate_for(&RuntimeStatusRequest {
                sandbox_id: sandbox.id.to_string(),
                generation,
                home_identity: source.home_identity.clone(),
            })
            .map_err(adapter_failure)?;
        if !status.installation_ready
            || status.restore_pending
            || status.credential_ready != credentialed
            || status.agent_processes != 0
        {
            return Err(WorkerFailure::terminal_security(
                "retained runtime does not prove completed home recovery",
            ));
        }
        Ok(status)
    }

    pub(super) async fn verify_retained_policy_readback(
        &self,
        handle: &RuntimeHandle,
        transition: &layer2_core::sandbox::RetainedRuntimeTransition,
        boot_id: &str,
    ) -> Result<layer3_adapter::RuntimeStartupPolicyObservation, WorkerFailure> {
        let runtime = RuntimeStatusRequest {
            sandbox_id: transition.request.sandbox_id.to_string(),
            generation: transition.successor_generation,
            home_identity: transition.request.predecessor.home_identity.clone(),
        };
        self.retained_startup_policy()?
            .verify_retained_startup_policy(
                handle,
                &layer3_adapter::RetainedStartupPolicyReadback {
                    runtime: &runtime,
                    boot_id,
                    image: &transition.request.target_image,
                    config_digest: &transition.request.target_config_digest,
                    journal_recovery: transition.request.journal_recovery,
                },
            )
            .await
            .map_err(|_| {
                WorkerFailure::retryable(
                    "retained runtime startup policy could not be confirmed on the admitted boot",
                )
            })
    }

    pub(super) async fn accept_lifecycle_home(
        &self,
        sandbox: Sandbox,
        runtime: &RuntimeHandle,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let generation = sandbox
            .runtime
            .as_ref()
            .and_then(|binding| binding.provision_generation)
            .ok_or_else(|| {
                SandboxStoreError::Corrupt("native runtime generation is absent".into())
            })?;
        if let Err(failure) = self
            .verify_native_runtime(&sandbox, runtime, generation, true)
            .await
        {
            return self.handle_lifecycle_failure(sandbox, failure, now).await;
        }
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_home_restore(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }
}

fn missing_source() -> WorkerFailure {
    WorkerFailure::terminal_security("the exact retained home recovery source is unavailable")
}
