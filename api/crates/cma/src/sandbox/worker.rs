use std::future::Future;
use std::pin::Pin;
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

mod authority;
mod credential_rotation;
mod execute;
mod home;
mod initial_runtime;
mod install_failure;
pub(super) mod lifecycle;
mod provision_timing;
mod provisioning;
mod retained_retirement;
mod retirement;
mod retirement_credentials;
mod retry;
mod scheduling;
mod startup_hold;
mod support;
mod teardown;
mod teardown_timing;

pub(crate) const CREDENTIAL_ROTATION_AGE: Duration = Duration::from_secs(12 * 60 * 60);
pub(crate) const CREDENTIAL_EXPIRY_MARGIN: Duration = Duration::from_secs(5 * 60);
const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);
const WORK_BATCH_SIZE: u16 = 50;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SandboxDependencyError {
    #[error("sandbox agents are still stopping")]
    AgentsStopping,
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
    pub retirement_finalizer:
        Option<Arc<super::native_retirement_finalizer::NativeRetirementFinalizer>>,
    pub renewable_work: bool,
    pub work_retry_policy_enabled: bool,
    pub native_create_retry_enabled: bool,
    pub reopen_output_refusal_enabled: bool,
    pub park_runtime_update_sandbox: Option<SandboxId>,
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    /// Fixed default for fixtures composed without a released authority.
    pub runtime_version: String,
    /// The process's released default for new provisioning, when composed.
    pub released: Option<Arc<dyn layer3_adapter::ReleasedRuntimeSource>>,
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
            retirement_finalizer: None,
            renewable_work: false,
            home_policy: None,
            work_retry_policy_enabled: false,
            native_create_retry_enabled: false,
            reopen_output_refusal_enabled: false,
            park_runtime_update_sandbox: None,
            execution: Default::default(),
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            runtime_version: runtime_version.into(),
            released: None,
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
            policy,
        }
    }

    pub fn with_replicas(mut self, enabled: bool) -> Self {
        self.renewable_work = enabled;
        if enabled {
            self.lease_duration =
                Duration::from_secs(layer2_core::sandbox::store::WORK_LEASE_SECONDS);
        }
        self
    }

    pub fn with_execution(
        mut self,
        execution: layer2_core::sandbox::ProvisioningExecution,
    ) -> Self {
        self.execution = execution;
        if !self.renewable_work {
            self.lease_duration = self.lease_duration.max(Duration::from_secs(
                layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS + 60,
            ));
        }
        self
    }

    /// Share the process's released-default authority instead of a fixed pin.
    pub(crate) fn with_released_runtime(
        mut self,
        released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    ) -> Self {
        self.released = Some(released);
        self
    }

    /// One release for an operation, when this worker has a released authority.
    pub(crate) fn release(&self) -> Option<layer3_adapter::ReleasedRuntime> {
        self.released.as_ref().map(|released| released.snapshot())
    }

    /// One release for a provisioning: the snapshot that renders it and the
    /// version recorded for `spec`, which keeps any image it already names.
    pub(crate) fn release_for(
        &self,
        spec: &layer3_adapter::SandboxSpec,
    ) -> (Option<layer3_adapter::ReleasedRuntime>, String) {
        let released = self.release();
        let version = spec
            .runtime_image
            .clone()
            .unwrap_or_else(|| self.default_version(released.as_ref()));
        (released, version)
    }

    /// The agent-lane default named by `release`, or the fixed fixture default.
    pub(crate) fn default_version(
        &self,
        release: Option<&layer3_adapter::ReleasedRuntime>,
    ) -> String {
        release.map_or_else(
            || self.runtime_version.clone(),
            |released| released.agent.clone(),
        )
    }

    pub fn with_home_policy(mut self, policy: Option<layer3_adapter::HomeVolumePolicy>) -> Self {
        self.home_policy = policy;
        self
    }

    pub(crate) fn with_native_create_retry(mut self, enabled: bool) -> Self {
        self.native_create_retry_enabled = enabled;
        self
    }

    pub(crate) fn with_work_retry_policy(mut self, enabled: bool) -> Self {
        self.work_retry_policy_enabled = enabled;
        self
    }
}

pub(crate) struct SandboxWorker {
    dependencies: SandboxWorkerDependencies,
    config: SandboxWorkerConfig,
    funding: Arc<dyn SandboxFundingAdapter>,
    reopen_relay: std::sync::OnceLock<std::sync::Weak<crate::agent_manager::relay::Relay>>,
    reopen_agents: std::sync::OnceLock<Arc<dyn layer2_core::sandbox::store::AgentStore>>,
    reopen_gates: std::sync::Mutex<lifecycle::ReopenGates>,
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
            reopen_gates: std::sync::Mutex::default(),
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
        let started = tokio::time::Instant::now();
        let stamp = rfc3339_at(now);
        let due = self
            .dependencies
            .store
            .due_sandbox_work(&stamp, WORK_BATCH_SIZE)
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            let at = if self.config.renewable_work {
                now + started.elapsed()
            } else {
                now
            };
            match self.run_scheduled(&id, at).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(completed), Err)
    }

    /// Run one claimed item behind a boxed boundary so callers do not inherit
    /// the full execution and renewal state-machine stack frame.
    pub fn run_one(
        &self,
        id: &SandboxId,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<bool, SandboxStoreError>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move { self.run_one_inner(&id, now).await })
    }

    async fn run_one_inner(
        &self,
        id: &SandboxId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let claiming = tokio::time::Instant::now();
        let now_stamp = rfc3339_at(now);
        let lease_expires = rfc3339_at(now + self.config.lease_duration);
        let claiming_work = self.dependencies.store.claim_sandbox_work(
            id,
            &self.config.holder,
            now_stamp,
            lease_expires,
        );
        let granted = if self.config.renewable_work {
            tokio::time::timeout(
                Duration::from_secs(layer2_core::sandbox::store::WORK_RENEW_SECONDS),
                claiming_work,
            )
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable("work acquisition acknowledgement timed out".into())
            })??
        } else {
            claiming_work.await?
        };
        let Some(sandbox) = granted else {
            return Ok(false);
        };
        #[cfg(test)]
        diagnostics::executing();
        if self.config.renewable_work || sandbox.work.authority_protocol.is_some() {
            return Box::pin(self.run_renewable(sandbox, now, claiming)).await;
        }
        let claim_ms = claiming.elapsed().as_millis() as u64;
        layer2_core::agent_ownership::transport_observation_scope(Box::pin(
            self.run_claimed(sandbox, now, claim_ms),
        ))
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

    #[cfg(test)]
    pub(super) async fn resume_claim_for_test(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        layer2_core::agent_ownership::transport_observation_scope(self.run_claimed(sandbox, now, 0))
            .await
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
            if !matches!((&current.work.lease, &claimed.work.lease),
                (Some(current), Some(original)) if current.holder == original.holder
                    && current.fence == original.fence && current.expires_at >= original.expires_at)
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

#[cfg(test)]
mod diagnostics;
