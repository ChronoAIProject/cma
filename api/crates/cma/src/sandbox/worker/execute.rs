use super::*;

impl SandboxWorker {
    pub(super) async fn execute(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if sandbox.work.step == SandboxWorkStep::RunLifecycleOperation
            && sandbox
                .lifecycle
                .home_restore
                .as_ref()
                .is_some_and(|restore| {
                    restore.receipt.is_none()
                        || sandbox
                            .lifecycle
                            .operation
                            .as_ref()
                            .is_some_and(|operation| {
                                operation.authority().operation_id == restore.operation_id
                            })
                })
        {
            let waiting = support::WorkerFailure::new(
                "home_restore_wait_for_compatible_worker",
                "home restoration requires a compatible worker; pinned authority is retained",
                true,
            )
            .with_retry_kind(layer2_core::sandbox::work_retry::WorkRetryFailure::Waiting);
            return self.handle_lifecycle_failure(sandbox, waiting, now).await;
        }
        // Re-read before any retirement side effect, including channel teardown.
        // Admission requires old-worker exclusion; an existing hold never expires.
        if sandbox.status == layer2_core::sandbox::SandboxStatus::Retiring
            || sandbox.lifecycle.retirement_archive.is_some()
        {
            let current = self
                .dependencies
                .store
                .get_sandbox(&sandbox.id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            if current.retirement_archive_pending() || current.revision != sandbox.revision {
                return Err(SandboxStoreError::Conflict);
            }
        }
        // A replacement is funded only after its exact generation and home
        // are durably reserved. Cleanup must remain available without credit.
        if sandbox.runtime.is_some()
            && !matches!(
                sandbox.status,
                layer2_core::sandbox::SandboxStatus::Pausing
                    | layer2_core::sandbox::SandboxStatus::Retiring
                    | layer2_core::sandbox::SandboxStatus::Rotating
            )
        {
            if let Err(error) = self.funding.ensure_coverage(&sandbox, now).await {
                return self
                    .retry_failure(sandbox, super::support::funding_failure(error), now)
                    .await;
            }
        }
        if matches!(
            sandbox.work.step,
            SandboxWorkStep::StopAgents
                | SandboxWorkStep::RevokeRetirementCredentials
                | SandboxWorkStep::TeardownRuntime
                | SandboxWorkStep::MarkRetired
                | SandboxWorkStep::RetryRetiredRevocations
                | SandboxWorkStep::WaitForPurge
                | SandboxWorkStep::Purge
        ) {
            match self
                .dependencies
                .credentials
                .retire_channel(&sandbox, self.dependencies.agents.as_ref(), now)
                .await
            {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    return self
                        .retry_failure(sandbox, support::credential_failure(error), now)
                        .await
                }
            }
        }
        if sandbox.lifecycle.checkpoint_capture.is_some() {
            return self
                .retry_failure(
                    sandbox,
                    support::WorkerFailure::retryable("checkpoint writer resumption is pending"),
                    now,
                )
                .await;
        }
        let requires_adoption = match sandbox.work.step {
            SandboxWorkStep::RunInstall
            | SandboxWorkStep::DeliverInitialCredential
            | SandboxWorkStep::DeliverRotatedCredential => true,
            SandboxWorkStep::CreateProfileWorkspace(_) => {
                self.dependencies.workspaces.requires_runtime_adoption()
            }
            _ => false,
        };
        if requires_adoption {
            let inspection_started = std::time::Instant::now();
            let attached = crate::sandbox::runtime_attachment::adopt_bound_runtime(
                self.dependencies.adapter.as_ref(),
                &sandbox,
                &self.config.deployment_id,
            )
            .await;
            tracing::info!(
                sandbox_id = %sandbox.id,
                step = ?sandbox.work.step,
                elapsed_ms = inspection_started.elapsed().as_millis() as u64,
                succeeded = attached.is_ok(),
                "Sandbox runtime inspection completed"
            );
            if let Err(error) = attached {
                return self
                    .handle_failure(sandbox, support::adapter_failure(error), now)
                    .await;
            }
        }
        self.execute_step(sandbox, now).await
    }

    // Keep the selected work future off the dispatch frame, as AgentWorker
    // does. Provisioning and teardown otherwise accumulate large nested poll
    // frames even when execute itself is boxed by its caller.
    fn execute_step(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<(), SandboxStoreError>> + Send + '_>> {
        match sandbox.work.step {
            SandboxWorkStep::ProvisionRuntime => Box::pin(self.provision(sandbox, now)),
            SandboxWorkStep::RunInstall => Box::pin(self.install(sandbox, now)),
            SandboxWorkStep::MintInitialCredential | SandboxWorkStep::MintRotatedCredential => {
                Box::pin(self.mint(sandbox, now))
            }
            SandboxWorkStep::DeliverInitialCredential
            | SandboxWorkStep::DeliverRotatedCredential => Box::pin(self.deliver(sandbox, now)),
            SandboxWorkStep::CreateProfileWorkspace(index) => {
                Box::pin(self.create_workspace(sandbox, index, now))
            }
            SandboxWorkStep::Activate => Box::pin(self.activate(sandbox, now)),
            SandboxWorkStep::WaitForCredentialRotation => {
                Box::pin(self.begin_rotation(sandbox, now))
            }
            SandboxWorkStep::RevokeRotatedCredential => Box::pin(self.revoke_rotated(sandbox, now)),
            SandboxWorkStep::FinishCredentialRotation => {
                Box::pin(self.finish_rotation(sandbox, now))
            }
            SandboxWorkStep::RunLifecycleOperation => Box::pin(self.run_lifecycle(sandbox, now)),
            SandboxWorkStep::StopAgents => Box::pin(self.stop_agents(sandbox, now)),
            SandboxWorkStep::RevokeRetirementCredentials => {
                Box::pin(self.revoke_for_retirement(sandbox, now))
            }
            SandboxWorkStep::TeardownRuntime => Box::pin(self.teardown(sandbox, now)),
            SandboxWorkStep::MarkRetired => Box::pin(self.mark_retired(sandbox, now)),
            SandboxWorkStep::RetryRetiredRevocations => {
                Box::pin(self.retry_retired_revoke(sandbox, now))
            }
            SandboxWorkStep::WaitForPurge => Box::pin(self.ready_to_purge(sandbox, now)),
            SandboxWorkStep::Purge => Box::pin(self.purge(sandbox, now)),
            SandboxWorkStep::None => Box::pin(self.fail_corrupt_claim(sandbox, now)),
        }
    }
}
