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

/// The immutable profile keeps the qualified image selected by this creation
/// operation even after the Agent release advances.
pub(super) fn managed_desktop_creation_release(
    sandbox: &Sandbox,
) -> Option<layer3_adapter::ReleasedRuntime> {
    if !sandbox.has_managed_desktop_image() {
        return None;
    }
    Some(layer3_adapter::ReleasedRuntime {
        revision: 0,
        agent: sandbox.profile.content.runtime_image.clone()?,
        machine: None,
        lane_revisions: None,
    })
}

impl SandboxWorker {
    pub(super) async fn initial_runtime_release(
        &self,
        sandbox: &Sandbox,
        spec: &layer3_adapter::SandboxSpec,
        generation: u64,
    ) -> Result<(Option<layer3_adapter::ReleasedRuntime>, String), SandboxStoreError> {
        let (mut released, mut runtime_version) = self.config.release_for(spec);
        if let Some(admitted) = managed_desktop_creation_release(sandbox) {
            if spec.runtime_image.as_deref() == Some(admitted.agent.as_str()) {
                return Ok((Some(admitted), runtime_version));
            }
        }
        if spec.runtime_image.is_none()
            && released
                .as_ref()
                .is_some_and(|release| release.lane_revisions.is_some())
        {
            // Native pooled homes and explicit images already have a durable
            // pin. A legacy dispatch keeps the bootstrap image across restarts.
            runtime_version = self
                .dependencies
                .store
                .reserve_initial_runtime_image(sandbox, generation, &self.config.runtime_version)
                .await?;
            released = Some(layer3_adapter::ReleasedRuntime {
                revision: 0,
                agent: runtime_version.clone(),
                machine: None,
                lane_revisions: None,
            });
        }
        Ok((released, runtime_version))
    }

    /// None means publication/failure was handled. Some reserves a replacement
    /// generation durably, preserving the ordinary lost-warm-runtime path.
    pub(in crate::sandbox) async fn adopt_initial_runtime(
        &self,
        mut sandbox: Sandbox,
        completed_digest: Option<&str>,
        now: SystemTime,
    ) -> Result<Option<Sandbox>, SandboxStoreError> {
        let prepared_installation = sandbox.work.prepared_installation_digest.is_some();
        let binding = sandbox
            .runtime
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let adoption = RuntimeAdoptionRequest::new(
            binding.provision_generation.unwrap_or(1),
            RuntimeHandle::new(&binding.runtime_id),
            sandbox_spec(&sandbox, &self.config.deployment_id)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
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
                if prepared_installation {
                    sandbox.work.prepared_installation_digest = None;
                }
                self.advance_status(sandbox, SandboxStatus::Installing, step, now)
                    .await?;
                Ok(None)
            }
            Ok(None) => {
                // No owner credential has been delivered. Persist the next
                // generation before replacing a lost, prebound runtime.
                let expected = sandbox.revision;
                sandbox.runtime = None;
                sandbox.work.prepared_installation_digest = None;
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
