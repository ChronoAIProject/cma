//! Adopt durable runtime bindings before workers run, then reconcile continuously.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use layer2_core::sandbox::store::{
    SandboxMaintenanceStore, SandboxReconciliationStore, SandboxRuntimeLossOutcome,
    SandboxRuntimeLossRequest, SandboxStore, SandboxStoreError,
};
use layer2_core::sandbox::{AgentService, Sandbox, SandboxPolicySource, SandboxStatus};
use layer2_core::{rfc3339_at, seconds_from_rfc3339, SandboxId};
use layer3_adapter::binding_diagnostics::{adapter_outcome, measure, Outcome, Stage};
use layer3_adapter::{RuntimeAdoptionRequest, RuntimeObservation, SandboxAdapter};

use self::transient::provider_error;
use super::runtime_mapping::sandbox_spec;

#[path = "reconciliation_supervision.rs"]
mod supervision;

#[path = "reconciliation_startup.rs"]
mod startup;

#[path = "reconciliation_interruption.rs"]
mod interruption;

#[path = "reconciliation_homes.rs"]
mod homes;

#[path = "reconciliation_updates.rs"]
mod updates;

#[path = "reconciliation_recovery.rs"]
mod recovery;

#[path = "reconciliation_authority.rs"]
mod authority;
#[path = "reconciliation_binding_holds.rs"]
mod binding_holds;
#[path = "reconciliation_diagnostics.rs"]
mod diagnostics;
#[path = "reconciliation_loss_observation.rs"]
mod loss_observation;
#[path = "reconciliation_pending_homes.rs"]
mod pending_homes;
#[path = "reconciliation_platform.rs"]
pub(crate) mod platform;
#[path = "reconciliation_sweep.rs"]
mod sweep;
#[path = "reconciliation_telemetry.rs"]
mod telemetry;
#[path = "reconciliation_transient.rs"]
mod transient;

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
    released: Option<Arc<dyn layer3_adapter::ReleasedRuntimeSource>>,
    failed_runtime_recovery: Option<Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>>,
    recovery_schedule: tokio::sync::Mutex<recovery::Schedule>,
    progress: tokio::sync::Mutex<Option<sweep::Progress>>,
    binding_holds: Arc<binding_holds::BindingHolds>,
    startup_native_home_hold_enabled: bool,
    platform_spend: Option<Arc<dyn platform::PlatformSpendSweep>>,
    platform_schedule: platform::PlatformSpendSchedule,
    background_work: crate::background::BackgroundWork,
    resume_background: bool,
    runtime_loss_bounded_proof_enabled: bool,
    pending_homes: Option<Arc<dyn pending_homes::PendingHomesSweep>>,
    pending_homes_schedule: tokio::sync::Mutex<pending_homes::PendingHomesSchedule>,
}

impl SandboxReconciler {
    #[cfg(test)]
    pub(crate) fn binding_is_held(&self, id: &SandboxId) -> bool {
        self.binding_holds.contains(id.as_str())
    }

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
            released: None,
            failed_runtime_recovery: None,
            recovery_schedule: tokio::sync::Mutex::new(recovery::Schedule::default()),
            progress: tokio::sync::Mutex::new(None),
            binding_holds: Default::default(),
            startup_native_home_hold_enabled: false,
            platform_spend: None,
            platform_schedule: Default::default(),
            background_work: crate::background::BackgroundWork::new(
                crate::background::shared(),
                "sandbox_reconciliation",
                Duration::from_secs(30),
                Duration::from_secs(15),
            ),
            resume_background: false,
            runtime_loss_bounded_proof_enabled: false,
            pending_homes: None,
            pending_homes_schedule: Default::default(),
        }
    }

    pub(crate) fn with_background_work(mut self, work: crate::background::BackgroundWork) -> Self {
        self.background_work = work;
        self
    }

    pub(crate) fn with_background_resumption(mut self, enabled: bool) -> Self {
        self.resume_background = enabled;
        self
    }

    pub(crate) fn with_binding_hold_metrics(self, enabled: bool) -> Self {
        self.binding_holds.set_metrics(enabled);
        self
    }

    pub(crate) fn binding_diagnostics(&self) -> Arc<dyn crate::binding_diagnostics::LocalSource> {
        self.binding_holds.enable_diagnostics();
        self.binding_holds.clone()
    }

    pub(crate) fn with_profile_agents(mut self, agents: Arc<AgentService>) -> Self {
        self.profile_agents = Some(agents);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_runtime_version(self, runtime_version: String) -> Self {
        self.with_released_runtime(Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
            runtime_version,
            None,
        )))
    }

    /// Daily updates target the source's agent-lane release, read per Sandbox.
    pub(crate) fn with_released_runtime(
        mut self,
        released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    ) -> Self {
        self.released = Some(released);
        self
    }

    pub(crate) fn with_failed_runtime_recovery(
        mut self,
        service: Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>,
    ) -> Self {
        self.failed_runtime_recovery = Some(service);
        self
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
    ) -> Result<bool, SandboxStoreError> {
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
            return Ok(true);
        }
        let Some(binding) = &sandbox.runtime else {
            if sandbox.requires_runtime_loss_recovery() {
                // The durable row is the only absence proof on this path. Take
                // its boundary immediately before the optional diagnostic and
                // carry that exact evidence into the CAS.
                let boundary = fresh_recovery_boundary(sandbox, now);
                if boundary.stale {
                    return Ok(false);
                }
                return self.recover_missing(sandbox, boundary.observed_at).await;
            }
            return Ok(true);
        };
        if let Some(observation) = inventory
            .iter()
            .find(|item| item.handle.0 == binding.runtime_id)
        {
            if self.adopt(sandbox, observation, now).await? {
                return Ok(true);
            }
        }
        // The inventory can predate a concurrently persisted runtime binding.
        // Absence is only actionable after a fresh provider observation.
        // Capture the boundary before that provider read: an Agent update
        // arriving while it is in flight must supersede this evidence rather
        // than being hidden by a later wall-clock timestamp.
        let boundary = fresh_recovery_boundary(sandbox, now);
        let fresh = measure(
            Stage::FinalLossObservation,
            self.adapter.runtime_inventory(),
            adapter_outcome,
        )
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
            return Ok(true);
        }
        if let Some(observation) = fresh
            .iter()
            .find(|item| item.handle.0 == binding.runtime_id)
        {
            if self.adopt(sandbox, observation, now).await? {
                return Ok(true);
            }
        }
        if boundary.stale {
            // A durable writer or caller floor is ahead of the provider read.
            // That read cannot authorize recovery; retain the hold and retry
            // after a new boundary instead of dating it in the future.
            return Ok(false);
        }
        if sandbox.requires_runtime_loss_recovery() {
            return self.recover_missing(sandbox, boundary.observed_at).await;
        }
        Ok(true)
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
            sandbox_spec(sandbox, &self.deployment_id)
                .map_err(|error| corrupt(&error.to_string()))?,
        )
        .with_expected_image(&binding.runtime_version);
        let adopted = measure(
            Stage::Adoption,
            self.adapter.adopt_runtime(&request),
            |result| match result {
                Ok(None) => Outcome::Absent,
                _ => adapter_outcome(result),
            },
        )
        .await;
        if self.binding_result(sandbox, adopted).await?.is_none() {
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
            let accepted = measure(
                Stage::Status,
                async {
                    let status = self.adapter.runtime_status(&observation.handle).await;
                    let status = self.binding_result(sandbox, status).await?;
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
                    Ok(true)
                },
                |result| match result {
                    Ok(false) => Outcome::Refused,
                    _ => diagnostics::store_outcome(result),
                },
            )
            .await?;
            if !accepted {
                return Ok(false);
            }
        }

        if next != *sandbox {
            measure(
                Stage::Transaction,
                self.store.save_sandbox(sandbox.revision, next),
                diagnostics::store_outcome,
            )
            .await?;
        } else {
            // An already recorded boot needs no write CAS. Still fence the
            // current attempt before releasing its hold: status may have
            // completed after another durable writer changed the parent.
            measure(
                Stage::DurableRead,
                async {
                    if self.store.get_sandbox(&sandbox.id).await?.as_ref() != Some(sandbox) {
                        return Err(SandboxStoreError::Conflict);
                    }
                    Ok(())
                },
                diagnostics::store_outcome,
            )
            .await?;
        }
        Ok(true)
    }

    async fn binding_result<T>(
        &self,
        sandbox: &Sandbox,
        result: layer3_adapter::Result<T>,
    ) -> Result<T, SandboxStoreError> {
        let error = match result {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if error.is_native_home_identity_refusal() {
            let current = measure(
                Stage::DurableRead,
                self.store.get_sandbox(&sandbox.id),
                diagnostics::store_outcome,
            )
            .await?;
            if current.is_some_and(|row| {
                row.revision > sandbox.revision
                    && (row.runtime != sandbox.runtime
                        || row.lifecycle.suspended_runtime != sandbox.lifecycle.suspended_runtime
                        || row.lifecycle.home != sandbox.lifecycle.home)
            }) {
                return Err(SandboxStoreError::Conflict);
            }
            return Err(transient::native_home_identity(error));
        }
        Err(provider_error(error))
    }
}

#[derive(Clone, Copy)]
struct RecoveryBoundary {
    observed_at: SystemTime,
    stale: bool,
}

fn fresh_recovery_boundary(sandbox: &Sandbox, floor: SystemTime) -> RecoveryBoundary {
    let observed_at = SystemTime::now();
    let durable = seconds_from_rfc3339(&sandbox.updated_at)
        .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH);
    RecoveryBoundary {
        observed_at,
        stale: floor > observed_at || durable > observed_at,
    }
}

fn corrupt(detail: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(detail.into())
}
