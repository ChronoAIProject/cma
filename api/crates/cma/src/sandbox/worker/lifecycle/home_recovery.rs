use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{Sandbox, SandboxStoreError};
use layer3_adapter::{RuntimeHandle, RuntimeStatus, RuntimeStatusRequest};

use super::super::support::{adapter_failure, WorkerFailure};
use super::super::SandboxWorker;
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    pub(super) async fn verify_lifecycle_home(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
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
        for observation in self
            .dependencies
            .adapter
            .runtime_inventory()
            .await
            .map_err(adapter_failure)?
            .iter()
            .filter(|observation| old(observation))
        {
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
                let request = crate::sandbox::runtime_attachment::retained_runtime_request(
                    sandbox,
                    binding,
                    &self.config.deployment_id,
                )
                .map_err(adapter_failure)?;
                self.dependencies
                    .adapter
                    .verify_suspended_runtime(&request)
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
