//! Adopt durable runtime bindings before workers run, then reconcile continuously.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{
    SandboxMaintenanceStore, SandboxReconciliationStore, SandboxRuntimeLossRequest, SandboxStore,
    SandboxStoreError,
};
use layer2_core::sandbox::{AgentService, Sandbox, SandboxPolicySource, SandboxStatus};
use layer2_core::{rfc3339_at, SandboxId};
use layer3_adapter::{RuntimeAdoptionRequest, RuntimeObservation, SandboxAdapter};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::runtime_mapping::sandbox_spec;

const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(15);

#[path = "reconciliation_interruption.rs"]
mod interruption;

#[path = "reconciliation_homes.rs"]
mod homes;

#[path = "reconciliation_updates.rs"]
mod updates;

#[path = "reconciliation_authority.rs"]
mod authority;
#[path = "reconciliation_binding_holds.rs"]
mod binding_holds;
#[path = "reconciliation_sweep.rs"]
mod sweep;
#[path = "reconciliation_telemetry.rs"]
mod telemetry;

pub(crate) trait ReconciliationStore:
    SandboxReconciliationStore + SandboxMaintenanceStore + SandboxStore
{
}

impl<T> ReconciliationStore for T where
    T: SandboxReconciliationStore + SandboxMaintenanceStore + SandboxStore
{
}

pub(crate) struct SandboxReconciler {
    store: Arc<dyn ReconciliationStore>,
    adapter: Arc<dyn SandboxAdapter>,
    policy: Arc<dyn SandboxPolicySource>,
    deployment_id: String,
    profile_agents: Option<Arc<AgentService>>,
    runtime_version: Option<String>,
    failed_runtime_recovery: Option<Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>>,
    recovery_schedule_cursor: std::sync::atomic::AtomicUsize,
    progress: tokio::sync::Mutex<Option<sweep::Progress>>,
    binding_holds: binding_holds::BindingHolds,
}

impl SandboxReconciler {
    pub(crate) fn new(
        store: Arc<dyn ReconciliationStore>,
        adapter: Arc<dyn SandboxAdapter>,
        policy: Arc<dyn SandboxPolicySource>,
        deployment_id: String,
    ) -> Self {
        Self {
            store,
            adapter,
            policy,
            deployment_id,
            profile_agents: None,
            runtime_version: None,
            failed_runtime_recovery: None,
            recovery_schedule_cursor: std::sync::atomic::AtomicUsize::new(0),
            progress: tokio::sync::Mutex::new(None),
            binding_holds: Default::default(),
        }
    }

    pub(crate) fn with_profile_agents(mut self, agents: Arc<AgentService>) -> Self {
        self.profile_agents = Some(agents);
        self
    }

    pub(crate) fn with_runtime_version(mut self, runtime_version: String) -> Self {
        self.runtime_version = Some(runtime_version);
        self
    }

    pub(crate) fn with_failed_runtime_recovery(
        mut self,
        service: Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>,
    ) -> Self {
        self.failed_runtime_recovery = Some(service);
        self
    }

    /// Fail closed before starting any worker if either authority is incomplete.
    pub(crate) async fn initialize(&self, now: SystemTime) -> Result<(), SandboxStoreError> {
        let inventory = self
            .adapter
            .runtime_inventory()
            .await
            .map_err(provider_error)?;
        let authority = self.authority().await?;
        for sandbox in authority.values() {
            self.reconcile_binding(sandbox, &inventory, now).await?;
        }
        for observation in &inventory {
            self.reconcile_runtime(observation).await?;
        }
        self.reconcile_homes().await
    }

    // Each resumed cleanup item repeats this complete pool-first/owned fence.
    async fn reconcile_runtime(
        &self,
        observation: &RuntimeObservation,
    ) -> Result<(), SandboxStoreError> {
        let id = SandboxId(observation.sandbox_id.clone());
        // Read pool authority first: a concurrent claim publishes its owned
        // Sandbox atomically, before the following fresh owned-record read.
        if self
            .store
            .unowned_runtime_authority(&id, observation.generation, observation.handle.as_str())
            .await?
        {
            return Ok(());
        }
        if let Some(current) = self.store.get_sandbox(&id).await? {
            current
                .validate()
                .map_err(|error| corrupt(&error.to_string()))?;
            // A settled pause/retirement cannot bind an equal generation;
            // any later revive reserves a newer one before provider I/O.
            let settled_unbound = current.runtime.is_none()
                && matches!(
                    current.status,
                    SandboxStatus::Paused | SandboxStatus::Retiring | SandboxStatus::Retired
                )
                && current.lifecycle.runtime_provision.is_none()
                && current.lifecycle.operation.is_none()
                && current.lifecycle.checkpoint_capture.is_none();
            if current
                .lifecycle
                .suspended_runtime
                .as_ref()
                .is_some_and(|binding| binding.runtime_id == observation.handle.0)
                || observation.generation > current.lifecycle.runtime_generation_high_water
                || (observation.generation == current.lifecycle.runtime_generation_high_water
                    && !settled_unbound)
                || current.runtime.as_ref().is_some_and(|binding| {
                    binding.runtime_id == observation.handle.0
                        || binding.provision_generation.is_none()
                })
            {
                return Ok(());
            }
            // Older recovery records can reserve a successor generation
            // without retaining the original provider binding. Its object
            // still owns the home; only explicit retirement may delete it.
            if observation.handle.as_str().starts_with("agent-sandbox:")
                && current
                    .lifecycle
                    .home
                    .as_ref()
                    .is_some_and(|home| !home.released)
            {
                return Ok(());
            }
        }
        self.adapter
            .reclaim_runtime(observation)
            .await
            .map_err(provider_error)?;
        Ok(())
    }

    async fn authority(&self) -> Result<BTreeMap<SandboxId, Sandbox>, SandboxStoreError> {
        let mut authority = authority::Authority::default();
        loop {
            let page = self
                .store
                .sandbox_runtime_authority(&authority.request)
                .await?;
            if authority.accept(page)? {
                return Ok(authority.items);
            }
        }
    }

    async fn reconcile_binding(
        &self,
        sandbox: &Sandbox,
        inventory: &[RuntimeObservation],
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // A durable suspension intent may have stopped the Pod before its DB
        // acknowledgement. Let the same worker retry and verify Pod absence.
        if super::runtime_attachment::native_pause_teardown(sandbox)
            || (sandbox.status == SandboxStatus::Retiring
                && sandbox.lifecycle.home.is_some()
                && sandbox
                    .runtime
                    .as_ref()
                    .is_some_and(|binding| binding.runtime_id.starts_with("agent-sandbox:")))
        {
            // Native retirement also needs the original UID after CRD disappearance:
            // only its delete waiter can prove the last owned Pod has stopped.
            return Ok(());
        }
        let Some(binding) = &sandbox.runtime else {
            if sandbox.requires_runtime_loss_recovery() {
                self.recover_missing(sandbox, now).await?;
            }
            return Ok(());
        };
        if let Some(observation) = inventory
            .iter()
            .find(|item| item.handle.0 == binding.runtime_id)
        {
            if self.adopt(sandbox, observation, now).await? {
                return Ok(());
            }
        }
        // The inventory can predate a concurrently persisted runtime binding.
        // Absence is only actionable after a fresh provider observation.
        let fresh = self
            .adapter
            .runtime_inventory()
            .await
            .map_err(provider_error)?;
        self.binding_holds.observe_inventory(sandbox, &fresh);
        if sandbox.status == SandboxStatus::Retiring
            && fresh.iter().any(|item| {
                item.handle.0 == binding.runtime_id || item.sandbox_id == sandbox.id.as_str()
            })
        {
            // An unhealthy or terminating runtime can still own processes and
            // writers. Retirement may settle them only after provider absence.
            return Ok(());
        }
        if let Some(observation) = fresh
            .iter()
            .find(|item| item.handle.0 == binding.runtime_id)
        {
            if self.adopt(sandbox, observation, now).await? {
                return Ok(());
            }
        }
        if sandbox.requires_runtime_loss_recovery() {
            self.recover_missing(sandbox, now).await?;
        }
        Ok(())
    }

    async fn adopt(
        &self,
        sandbox: &Sandbox,
        observation: &RuntimeObservation,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        if observation.sandbox_id != sandbox.id.as_str() {
            return Err(corrupt(
                "bound provider runtime belongs to a different Sandbox",
            ));
        }
        if !observation.lifecycle.is_running() {
            return Ok(false);
        }
        let binding = sandbox
            .runtime
            .as_ref()
            .expect("only bound runtimes are adopted");
        if binding
            .provision_generation
            .is_some_and(|expected| expected != observation.generation)
        {
            return Err(corrupt(
                "bound provider runtime generation disagrees with durable authority",
            ));
        }
        let request = RuntimeAdoptionRequest::new(
            observation.generation,
            observation.handle.clone(),
            sandbox_spec(sandbox, &self.deployment_id),
        )
        .with_expected_image(&binding.runtime_version);
        if self
            .adapter
            .adopt_runtime(&request)
            .await
            .map_err(provider_error)?
            .is_none()
        {
            return Ok(false);
        }
        let mut next = sandbox.clone();
        if binding.provision_generation.is_none() {
            next.record_adopted_runtime_generation(
                binding,
                observation.generation,
                rfc3339_at(now),
            )
            .map_err(|error| corrupt(&error.to_string()))?;
        }
        if sandbox.lifecycle.home.is_some() {
            let status = self
                .adapter
                .runtime_status(&observation.handle)
                .await
                .map_err(provider_error)?;
            if !next
                .runtime_process_matches(&status)
                .map_err(|error| corrupt(&error.to_string()))?
                || (matches!(
                    sandbox.status,
                    SandboxStatus::Active
                        | SandboxStatus::CredentialRotating
                        | SandboxStatus::Pausing
                ) && (!status.installation_ready || !status.credential_ready))
            {
                return Ok(false);
            }
            let expected = next.runtime.clone().expect("adopted runtime is bound");
            next.record_runtime_boot(&expected, &status, rfc3339_at(now))
                .map_err(|error| corrupt(&error.to_string()))?;
        }
        if next != *sandbox {
            self.store.save_sandbox(sandbox.revision, next).await?;
        }
        Ok(true)
    }

    async fn recover_missing(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let interruption_cause = interruption::observe(self.adapter.as_ref(), sandbox, now).await;
        self.store
            .recover_runtime_loss(SandboxRuntimeLossRequest {
                sandbox_id: sandbox.id.clone(),
                expected_revision: sandbox.revision,
                expected_runtime: sandbox.runtime.clone(),
                retain_runtime: sandbox.lifecycle.home.is_some()
                    && sandbox
                        .runtime
                        .as_ref()
                        .is_some_and(|binding| binding.runtime_id.starts_with("agent-sandbox:")),
                interruption_cause,
                observed_at: rfc3339_at(now),
            })
            .await?;
        Ok(())
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(RECONCILIATION_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                    _ = ticker.tick() => {
                        if let Err(error) = crate::background::sandbox_reconciliation(|| self.run_due(SystemTime::now())).await {
                            tracing::error!(error = %error, "sandbox runtime reconciliation failed");
                        }
                    }
                }
            }
        })
    }
}

fn provider_error(error: layer3_adapter::AdapterError) -> SandboxStoreError {
    SandboxStoreError::Unavailable(format!("sandbox runtime authority is unavailable: {error}"))
}

fn corrupt(detail: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(detail.into())
}
