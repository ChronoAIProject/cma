use super::*;

impl SandboxWorker {
    pub(super) async fn execute(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
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
        match sandbox.work.step {
            SandboxWorkStep::ProvisionRuntime => self.provision(sandbox, now).await,
            SandboxWorkStep::RunInstall => self.install(sandbox, now).await,
            SandboxWorkStep::MintInitialCredential | SandboxWorkStep::MintRotatedCredential => {
                self.mint(sandbox, now).await
            }
            SandboxWorkStep::DeliverInitialCredential
            | SandboxWorkStep::DeliverRotatedCredential => self.deliver(sandbox, now).await,
            SandboxWorkStep::CreateProfileWorkspace(index) => {
                self.create_workspace(sandbox, index, now).await
            }
            SandboxWorkStep::Activate => self.activate(sandbox, now).await,
            SandboxWorkStep::WaitForCredentialRotation => self.begin_rotation(sandbox, now).await,
            SandboxWorkStep::RevokeRotatedCredential => self.revoke_rotated(sandbox, now).await,
            SandboxWorkStep::FinishCredentialRotation => self.finish_rotation(sandbox, now).await,
            SandboxWorkStep::RunLifecycleOperation => {
                Box::pin(self.run_lifecycle(sandbox, now)).await
            }
            SandboxWorkStep::StopAgents => self.stop_agents(sandbox, now).await,
            SandboxWorkStep::RevokeRetirementCredentials => {
                self.revoke_for_retirement(sandbox, now).await
            }
            SandboxWorkStep::TeardownRuntime => self.teardown(sandbox, now).await,
            SandboxWorkStep::MarkRetired => self.mark_retired(sandbox, now).await,
            SandboxWorkStep::RetryRetiredRevocations => {
                self.retry_retired_revoke(sandbox, now).await
            }
            SandboxWorkStep::WaitForPurge => self.ready_to_purge(sandbox, now).await,
            SandboxWorkStep::Purge => self.purge(sandbox, now).await,
            SandboxWorkStep::None => self.fail_corrupt_claim(sandbox, now).await,
        }
    }
}
