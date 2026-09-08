use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::profiles::WorkspaceProfileRef;
use layer2_core::sandbox::store::{SandboxLifecycleStore, SandboxStoreError};
use layer2_core::sandbox::{
    AgentOperationService, AgentUpdateRecorder, Sandbox, SandboxPolicySource, SandboxWorkStep,
};
use layer2_core::{rfc3339_at, SandboxId};
use layer3_adapter::{RuntimeHandle, SandboxAdapter};

use super::credential::SandboxCredentialController;
use super::live_agents::LiveAgentRegistry;

#[cfg(test)]
use layer2_core::sandbox::SandboxPolicy;

mod lifecycle;
mod provisioning;
mod retirement;
mod support;

pub(crate) const CREDENTIAL_ROTATION_AGE: Duration = Duration::from_secs(12 * 60 * 60);
pub(crate) const CREDENTIAL_EXPIRY_MARGIN: Duration = Duration::from_secs(5 * 60);
const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);
const WORK_BATCH_SIZE: u16 = 50;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SandboxDependencyError {
    #[error("dependency is temporarily unavailable: {0}")]
    Transient(String),
    #[error("requested resources are unavailable: {0}")]
    ResourceQuota(String),
    #[error("dependency rejected lifecycle data: {0}")]
    Validation(String),
    #[error("dependency rejected lifecycle authority: {0}")]
    Security(String),
}

#[async_trait::async_trait]
pub(crate) trait ProfileWorkspaceProvisioner: Send + Sync {
    async fn create_profile_workspace(
        &self,
        sandbox: &Sandbox,
        runtime: &RuntimeHandle,
        profile: &WorkspaceProfileRef,
        index: u16,
        operation_id: &str,
    ) -> Result<(), SandboxDependencyError>;
}

#[async_trait::async_trait]
pub(crate) trait LiveAgentStopper: Send + Sync {
    async fn stop_live_agents(
        &self,
        sandbox: &Sandbox,
        runtime: Option<&RuntimeHandle>,
        operation_id: &str,
    ) -> Result<(), SandboxDependencyError>;
}

#[async_trait::async_trait]
pub(crate) trait CheckpointObjectPurger: Send + Sync {
    async fn purge_checkpoint_objects(
        &self,
        sandbox: &Sandbox,
        operation_id: &str,
    ) -> Result<(), SandboxDependencyError>;
}

pub(crate) struct SandboxWorkerDependencies {
    pub store: Arc<dyn SandboxLifecycleStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub credentials: Arc<dyn SandboxCredentialController>,
    pub workspaces: Arc<dyn ProfileWorkspaceProvisioner>,
    pub agents: Arc<dyn LiveAgentStopper>,
    pub checkpoints: Arc<dyn CheckpointObjectPurger>,
    pub registry: Arc<LiveAgentRegistry>,
    pub updates: Arc<dyn AgentUpdateRecorder>,
    pub operations: Arc<AgentOperationService>,
}

#[derive(Clone)]
pub(crate) struct SandboxWorkerConfig {
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    pub runtime_version: String,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
    pub policy: Arc<dyn SandboxPolicySource>,
}

impl SandboxWorkerConfig {
    #[cfg(test)]
    pub fn new(
        holder: impl Into<String>,
        deployment_id: impl Into<String>,
        runtime_version: impl Into<String>,
        policy: SandboxPolicy,
    ) -> Self {
        Self::with_policy_source(holder, deployment_id, runtime_version, Arc::new(policy))
    }

    pub fn with_policy_source(
        holder: impl Into<String>,
        deployment_id: impl Into<String>,
        runtime_version: impl Into<String>,
        policy: Arc<dyn SandboxPolicySource>,
    ) -> Self {
        Self {
            execution: Default::default(),
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            runtime_version: runtime_version.into(),
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
            policy,
        }
    }

    pub fn with_execution(
        mut self,
        execution: layer2_core::sandbox::ProvisioningExecution,
    ) -> Self {
        self.execution = execution;
        self.lease_duration = self.lease_duration.max(Duration::from_secs(
            layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS + 60,
        ));
        self
    }
}

pub(crate) struct SandboxWorker {
    dependencies: SandboxWorkerDependencies,
    config: SandboxWorkerConfig,
}

impl SandboxWorker {
    pub fn new(dependencies: SandboxWorkerDependencies, config: SandboxWorkerConfig) -> Arc<Self> {
        Arc::new(Self {
            dependencies,
            config,
        })
    }

    pub async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        let stamp = rfc3339_at(now);
        let due = self
            .dependencies
            .store
            .due_sandbox_work(&stamp, WORK_BATCH_SIZE)
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            match self.run_one(&id, now).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(completed), Err)
    }

    pub async fn run_one(
        &self,
        id: &SandboxId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let now_stamp = rfc3339_at(now);
        let lease_expires = rfc3339_at(now + self.config.lease_duration);
        let Some(sandbox) = self
            .dependencies
            .store
            .claim_sandbox_work(id, &self.config.holder, now_stamp, lease_expires)
            .await?
        else {
            return Ok(false);
        };
        Box::pin(self.execute(sandbox, now)).await?;
        Ok(true)
    }

    #[cfg(test)]
    pub(super) async fn execute_claimed_for_test(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        Box::pin(self.execute(sandbox, now)).await
    }

    async fn execute(&self, sandbox: Sandbox, now: SystemTime) -> Result<(), SandboxStoreError> {
        if sandbox.lifecycle.checkpoint_capture.is_some() {
            return self
                .retry_failure(
                    sandbox,
                    support::WorkerFailure::retryable("checkpoint writer resumption is pending"),
                    now,
                )
                .await;
        }
        if matches!(
            sandbox.work.step,
            SandboxWorkStep::RunInstall
                | SandboxWorkStep::DeliverInitialCredential
                | SandboxWorkStep::DeliverRotatedCredential
                | SandboxWorkStep::CreateProfileWorkspace(_)
        ) {
            if let Err(error) = super::runtime_attachment::adopt_bound_runtime(
                self.dependencies.adapter.as_ref(),
                &sandbox,
                &self.config.deployment_id,
            )
            .await
            {
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
