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
use super::funding::SandboxFundingAdapter;
use super::live_agents::LiveAgentRegistry;

#[cfg(test)]
use layer2_core::sandbox::SandboxPolicy;

mod execute;
mod home;
mod initial_runtime;
mod lifecycle;
mod provision_timing;
mod provisioning;
mod retained_retirement;
mod retirement;
mod scheduling;
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
    /// The durable workspace worker validates current parent and runtime
    /// authority itself. Other provisioners retain the caller's adoption check.
    fn requires_runtime_adoption(&self) -> bool {
        true
    }

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

pub(crate) struct SandboxWorkerDependencies {
    pub store: Arc<dyn SandboxLifecycleStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub credentials: Arc<dyn SandboxCredentialController>,
    pub workspaces: Arc<dyn ProfileWorkspaceProvisioner>,
    pub agents: Arc<dyn LiveAgentStopper>,
    pub registry: Arc<LiveAgentRegistry>,
    pub updates: Arc<dyn AgentUpdateRecorder>,
    pub operations: Arc<AgentOperationService>,
}

#[derive(Clone)]
pub(crate) struct SandboxWorkerConfig {
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
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
            home_policy: None,
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

    pub fn with_home_policy(mut self, policy: Option<layer3_adapter::HomeVolumePolicy>) -> Self {
        self.home_policy = policy;
        self
    }
}

pub(crate) struct SandboxWorker {
    dependencies: SandboxWorkerDependencies,
    config: SandboxWorkerConfig,
    funding: Arc<dyn SandboxFundingAdapter>,
    reopen_relay: std::sync::OnceLock<std::sync::Weak<crate::agent_manager::relay::Relay>>,
    reopen_agents: std::sync::OnceLock<Arc<dyn layer2_core::sandbox::store::AgentStore>>,
    reopen_gate: tokio::sync::Mutex<()>,
    pending_reopens: std::sync::Mutex<
        std::collections::BTreeMap<layer2_core::AgentId, lifecycle::PendingReopen>,
    >,
}

impl SandboxWorker {
    #[cfg(test)]
    pub fn new(dependencies: SandboxWorkerDependencies, config: SandboxWorkerConfig) -> Arc<Self> {
        Self::new_with_funding(
            dependencies,
            config,
            Arc::new(super::funding::NoopFundingAdapter),
        )
    }

    pub fn new_with_funding(
        dependencies: SandboxWorkerDependencies,
        config: SandboxWorkerConfig,
        funding: Arc<dyn SandboxFundingAdapter>,
    ) -> Arc<Self> {
        Arc::new(Self {
            dependencies,
            config,
            funding,
            reopen_relay: std::sync::OnceLock::new(),
            reopen_agents: std::sync::OnceLock::new(),
            reopen_gate: tokio::sync::Mutex::new(()),
            pending_reopens: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        })
    }

    pub(crate) fn install_reopen_relay(
        &self,
        relay: &Arc<crate::agent_manager::relay::Relay>,
        agents: Arc<dyn layer2_core::sandbox::store::AgentStore>,
    ) {
        assert!(self.reopen_agents.set(agents).is_ok());
        assert!(self.reopen_relay.set(Arc::downgrade(relay)).is_ok());
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
            match self.run_scheduled(&id, now).await {
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
        let claiming = tokio::time::Instant::now();
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
        let claim_ms = claiming.elapsed().as_millis() as u64;
        layer2_core::agent_ownership::transport_observation_scope(
            self.run_claimed(sandbox, now, claim_ms),
        )
        .await
    }

    async fn run_claimed(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
        claim_ms: u64,
    ) -> Result<bool, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let result = Box::pin(self.execute(sandbox.clone(), now)).await;
        tracing::info!(
            sandbox_id = %sandbox.id,
            step = ?sandbox.work.step,
            lifecycle_phase = ?scheduling::phase(&sandbox),
            claim_ms,
            execution_ms = started.elapsed().as_millis() as u64,
            published = result.is_ok(),
            "Sandbox work attempt completed"
        );
        if let Err(error) = result {
            if error != SandboxStoreError::Conflict
                || !self
                    .retry_conflicted_lifecycle(&sandbox, now + started.elapsed())
                    .await?
            {
                return Err(error);
            }
        }
        Ok(true)
    }

    /// A finished attempt must not leave its long capture lease blocking a retry
    /// after unrelated metadata cleanup wins the revision CAS. Re-read current
    /// state, preserve its changes, and release only our unchanged operation.
    async fn retry_conflicted_lifecycle(
        &self,
        claimed: &Sandbox,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        if claimed.work.step != SandboxWorkStep::RunLifecycleOperation {
            return Ok(false);
        }
        let started = tokio::time::Instant::now();
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(50 << (attempt - 1))).await;
            }
            let Some(current) = self.dependencies.store.get_sandbox(&claimed.id).await? else {
                return Ok(false);
            };
            if current.work.lease != claimed.work.lease
                || current.work.step != claimed.work.step
                || current.lifecycle.operation != claimed.lifecycle.operation
                || current.runtime != claimed.runtime
                || current.credential != claimed.credential
                || current.status != claimed.status
            {
                return Ok(false);
            }
            match self
                .handle_lifecycle_failure(
                    current,
                    support::WorkerFailure::retryable(
                        "lifecycle publication conflicted with concurrent metadata changes",
                    ),
                    now + started.elapsed(),
                )
                .await
            {
                Ok(()) => return Ok(true),
                // Retry publication can contend too. Re-read all guards before
                // releasing the same lease; an unknown commit never loops here.
                Err(SandboxStoreError::Conflict) => {}
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }

    async fn run_scheduled(
        &self,
        id: &SandboxId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let progressed = self.run_one(id, now).await?;
        if !progressed {
            return Ok(false);
        }
        // Follow committed local progress without waiting for another sweep.
        // Every effect still requires a fresh store-issued lease and fence.
        for _ in 0..16 {
            let Some(sandbox) = self.dependencies.store.get_sandbox(id).await? else {
                break;
            };
            if !scheduling::can_follow(&sandbox)
                || sandbox.work.attempts != 0
                || !sandbox.work.is_due(&rfc3339_at(now + started.elapsed()))
            {
                break;
            }
            if !self.run_one(id, now + started.elapsed()).await? {
                break;
            }
        }
        Ok(true)
    }

    #[cfg(test)]
    pub(super) async fn run_claimed_for_test(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        self.run_claimed(sandbox, now, 0).await
    }

    #[cfg(test)]
    pub(super) async fn execute_claimed_for_test(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        Box::pin(self.execute(sandbox, now)).await
    }
}
