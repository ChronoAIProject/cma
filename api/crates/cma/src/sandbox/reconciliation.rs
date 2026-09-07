//! Adopt durable runtime bindings before workers run, then reconcile continuously.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{
    PageRequest, SandboxImageRotationRequest, SandboxInactivityPauseRequest,
    SandboxMaintenanceOutcome, SandboxMaintenanceStore, SandboxReconciliationStore,
    SandboxRuntimeLossRequest, SandboxStore, SandboxStoreError,
};
use layer2_core::sandbox::{AgentService, Sandbox, SandboxPolicySource};
use layer2_core::{rfc3339_at, SandboxId};
use layer3_adapter::{RuntimeAdoptionRequest, RuntimeObservation, SandboxAdapter};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::runtime_mapping::sandbox_spec;

const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(15);

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
    runtime_version: String,
    profile_agents: Option<Arc<AgentService>>,
}

impl SandboxReconciler {
    pub(crate) fn new(
        store: Arc<dyn ReconciliationStore>,
        adapter: Arc<dyn SandboxAdapter>,
        policy: Arc<dyn SandboxPolicySource>,
        deployment_id: String,
        runtime_version: String,
    ) -> Self {
        Self {
            store,
            adapter,
            policy,
            deployment_id,
            runtime_version,
            profile_agents: None,
        }
    }

    pub(crate) fn with_profile_agents(mut self, agents: Arc<AgentService>) -> Self {
        self.profile_agents = Some(agents);
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
            let id = SandboxId(observation.sandbox_id.clone());
            if let Some(current) = self.store.get_sandbox(&id).await? {
                current
                    .validate()
                    .map_err(|error| corrupt(&error.to_string()))?;
                // The durable high-water mark cannot regress or bind an older
                // generation. Equal/newer observations may still be provisioning.
                if observation.generation >= current.lifecycle.runtime_generation_high_water
                    || current.runtime.as_ref().is_some_and(|binding| {
                        binding.runtime_id == observation.handle.0
                            || binding.provision_generation.is_none()
                    })
                {
                    continue;
                }
            }
            self.adapter
                .reclaim_runtime(observation)
                .await
                .map_err(provider_error)?;
        }
        Ok(())
    }

    async fn authority(&self) -> Result<BTreeMap<SandboxId, Sandbox>, SandboxStoreError> {
        let mut authority = BTreeMap::new();
        let mut request = PageRequest::default();
        let mut cursors = BTreeSet::new();
        loop {
            let page = self.store.sandbox_runtime_authority(&request).await?;
            let mut previous = request.cursor.as_deref();
            for sandbox in &page.items {
                sandbox
                    .validate()
                    .map_err(|error| corrupt(&error.to_string()))?;
                if previous.is_some_and(|id| id >= sandbox.id.as_str())
                    || authority.contains_key(&sandbox.id)
                {
                    return Err(corrupt("runtime authority scan is not strictly ordered"));
                }
                previous = Some(sandbox.id.as_str());
            }
            if page.items.len() > usize::from(request.limit)
                || page.next.as_ref().is_some_and(|next| {
                    previous != Some(next.as_str()) || !cursors.insert(next.clone())
                })
            {
                return Err(corrupt("runtime authority scan did not advance completely"));
            }
            authority.extend(
                page.items
                    .into_iter()
                    .map(|sandbox| (sandbox.id.clone(), sandbox)),
            );
            let Some(next) = page.next else {
                return Ok(authority);
            };
            request.cursor = Some(next);
        }
    }

    async fn reconcile_binding(
        &self,
        sandbox: &Sandbox,
        inventory: &[RuntimeObservation],
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
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
        if binding.provision_generation.is_none() {
            let mut next = sandbox.clone();
            next.record_adopted_runtime_generation(
                binding,
                observation.generation,
                rfc3339_at(now),
            )
            .map_err(|error| corrupt(&error.to_string()))?;
            self.store.save_sandbox(sandbox.revision, next).await?;
        }
        Ok(true)
    }

    async fn recover_missing(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.store
            .recover_runtime_loss(SandboxRuntimeLossRequest {
                sandbox_id: sandbox.id.clone(),
                expected_revision: sandbox.revision,
                expected_runtime: sandbox.runtime.clone(),
                observed_at: rfc3339_at(now),
            })
            .await?;
        Ok(())
    }

    pub(super) async fn run_due(&self, now: SystemTime) -> Result<(), SandboxStoreError> {
        self.initialize(now).await?;
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|error| {
                SandboxStoreError::Unavailable(format!(
                    "sandbox maintenance policy is unavailable: {error}"
                ))
            })?;
        let mut page = PageRequest::default();
        let mut seen = BTreeSet::new();
        loop {
            let candidates = self.store.sandbox_maintenance_candidates(&page).await?;
            for sandbox_id in candidates.items {
                if let Some(agents) = &self.profile_agents {
                    if let Err(error) = agents.reconcile_profile_agents(&sandbox_id).await {
                        tracing::error!(sandbox_id = %sandbox_id, error = %error, "automatic profile agent reconciliation failed");
                    }
                }
                let rotation = self
                    .store
                    .schedule_image_rotation(SandboxImageRotationRequest {
                        sandbox_id: sandbox_id.clone(),
                        requested_at: rfc3339_at(now),
                        expected_runtime_version: self.runtime_version.clone(),
                    })
                    .await?;
                if rotation == SandboxMaintenanceOutcome::NotEligible {
                    self.store
                        .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                            sandbox_id,
                            requested_at: rfc3339_at(now),
                            pause_after_seconds: policy.inactivity_pause_after_seconds,
                        })
                        .await?;
                }
            }
            let Some(next) = candidates.next else {
                return Ok(());
            };
            if !seen.insert(next.clone())
                || page
                    .cursor
                    .as_ref()
                    .is_some_and(|previous| previous >= &next)
            {
                return Err(corrupt("maintenance candidate scan did not advance"));
            }
            page.cursor = Some(next);
        }
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
                        if let Err(error) = self.run_due(SystemTime::now()).await {
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
