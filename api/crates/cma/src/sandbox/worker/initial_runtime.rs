//! Transfer an exact installation proof while adopting the initial runtime.
use std::time::SystemTime;

use layer2_core::sandbox::{Sandbox, SandboxStatus, SandboxStoreError, SandboxWorkStep};
use layer3_adapter::{AdapterError, InstallRequest, RuntimeAdoptionRequest, RuntimeHandle};

use super::{support::adapter_failure, SandboxWorker};
use crate::sandbox::{
    runtime_attachment::validate_bound_inspection, runtime_mapping::sandbox_spec,
};

pub(super) fn installation_request(sandbox: &Sandbox) -> Option<InstallRequest> {
    let execution = sandbox.work.execution?;
    Some(InstallRequest::with_timeouts(
        format!("sandbox-install-v1:{}", sandbox.id),
        sandbox.profile.content.software_install_commands.clone(),
        execution.command_timeout_seconds,
        execution.total_timeout_seconds,
        layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
    ))
}

impl SandboxWorker {
    /// None means publication/failure was handled. Some reserves a replacement
    /// generation durably, preserving the ordinary lost-warm-runtime path.
    pub(in crate::sandbox) async fn adopt_initial_runtime(
        &self,
        mut sandbox: Sandbox,
        completed_digest: Option<&str>,
        now: SystemTime,
    ) -> Result<Option<Sandbox>, SandboxStoreError> {
        let binding = sandbox
            .runtime
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let adoption = RuntimeAdoptionRequest::new(
            binding.provision_generation.unwrap_or(1),
            RuntimeHandle::new(&binding.runtime_id),
            sandbox_spec(&sandbox, &self.config.deployment_id),
        )
        .with_expected_image(&binding.runtime_version);
        let transfer = completed_digest.is_some_and(|digest| {
            installation_request(&sandbox).is_some_and(|request| request.request_sha256 == digest)
        });
        let inspection_started = std::time::Instant::now();
        let next = if transfer {
            self.inspect_initial_installation(&sandbox, &adoption).await
        } else {
            self.dependencies
                .adapter
                .adopt_runtime(&adoption)
                .await
                .map(|runtime| runtime.map(|_| SandboxWorkStep::RunInstall))
        };
        tracing::info!(
            sandbox_id = %sandbox.id,
            step = ?SandboxWorkStep::ProvisionRuntime,
            elapsed_ms = inspection_started.elapsed().as_millis() as u64,
            succeeded = next.is_ok(),
            installation_proof_matched = transfer,
            "Sandbox runtime inspection completed"
        );
        match next {
            Ok(Some(step)) => {
                self.advance_status(sandbox, SandboxStatus::Installing, step, now)
                    .await?;
                Ok(None)
            }
            Ok(None) => {
                // No owner credential has been delivered. Persist the next
                // generation before replacing a lost, prebound runtime.
                let expected = sandbox.revision;
                sandbox.runtime = None;
                sandbox.lifecycle.runtime_generation_high_water = sandbox
                    .lifecycle
                    .runtime_generation_high_water
                    .checked_add(1)
                    .ok_or(SandboxStoreError::Conflict)?;
                sandbox.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
                self.dependencies
                    .store
                    .save_sandbox(expected, sandbox.clone())
                    .await?;
                Ok(Some(sandbox))
            }
            Err(error) => {
                self.handle_failure(sandbox, adapter_failure(error), now)
                    .await?;
                Ok(None)
            }
        }
    }

    async fn inspect_initial_installation(
        &self,
        sandbox: &Sandbox,
        request: &RuntimeAdoptionRequest,
    ) -> layer3_adapter::Result<Option<SandboxWorkStep>> {
        let Some(inspected) = self.dependencies.adapter.inspect_runtime(request).await? else {
            return Ok(None);
        };
        validate_bound_inspection(sandbox, request.generation, &inspected)?;
        let status = inspected.status.as_ref().ok_or_else(|| {
            AdapterError::Rejected("completed installation requires process status".into())
        })?;
        if sandbox.runtime.as_ref().is_none_or(|binding| {
            binding.boot_id.as_ref() != Some(&status.boot_id)
                || binding.provision_generation != Some(status.generation)
        }) || sandbox.lifecycle.home.is_none()
            || !status.installation_ready
            || status.restore_pending
            || status.credential_ready
            || sandbox.credential.key_generation != 0
            || sandbox.credential.delivered
            || sandbox.credential.current.is_some()
            || sandbox.credential.pending.is_some()
        {
            return Err(AdapterError::Rejected(
                "completed installation does not match the initial runtime state".into(),
            ));
        }
        Ok(Some(SandboxWorkStep::MintInitialCredential))
    }
}
