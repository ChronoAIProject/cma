//! Startup-only overlap; all authority and cleanup barriers remain in order.
use super::{provider_error, SandboxReconciler};
use crate::startup_timing::{observe, Phase};
use layer2_core::sandbox::SandboxStoreError;
use std::time::SystemTime;

#[path = "reconciliation_startup_batch.rs"]
mod batch;
#[path = "reconciliation_startup_binding.rs"]
mod binding;
#[path = "reconciliation_startup_targets.rs"]
mod targets;

const BINDING_CONCURRENCY: usize = 8;

impl SandboxReconciler {
    /// General/reused callers without exclusive ownership keep the old serial path.
    #[cfg(test)]
    pub(crate) async fn initialize(&self, now: SystemTime) -> Result<(), SandboxStoreError> {
        self.initialize_with_exclusion(now, false, &|| false).await
    }

    /// Whether a binding hold currently protects this Sandbox.
    #[cfg(test)]
    pub(crate) fn binding_held(&self, id: &str) -> bool {
        self.binding_holds.contains(id)
    }

    #[cfg(test)]
    pub(crate) async fn initialize_before_workers(
        &self,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.initialize_before_workers_until(now, || false).await
    }

    /// Composition excludes other assignment-cache writers until this returns:
    /// runtime/preparation workers and HTTP execution admission follow this barrier.
    /// Durable authority can still change; its CAS/fresh reads remain mandatory.
    /// The headroom snapshot is not a reservation.
    pub(crate) async fn initialize_before_workers_until(
        &self,
        now: SystemTime,
        stopping: impl Fn() -> bool,
    ) -> Result<(), SandboxStoreError> {
        self.initialize_with_exclusion(now, true, &stopping).await
    }

    pub(crate) fn startup_binding_schedule(
        &self,
        authority: &std::collections::BTreeMap<
            layer2_core::SandboxId,
            layer2_core::sandbox::Sandbox,
        >,
        inventory: &[layer3_adapter::RuntimeObservation],
        exclusive: bool,
    ) -> (usize, &'static str) {
        if exclusive {
            match targets::independent(authority.values(), inventory) {
                Ok(handles) => match self.adapter.startup_binding_headroom(&handles) {
                    Some(headroom) if headroom >= authority.len() => (BINDING_CONCURRENCY, "none"),
                    Some(_) => (1, "capacity_limited"),
                    None => (1, "capacity_unknown"),
                },
                Err(reason) => (1, reason),
            }
        } else {
            (1, "nonexclusive_context")
        }
    }

    async fn initialize_with_exclusion(
        &self,
        now: SystemTime,
        exclusive: bool,
        stopping: &impl Fn() -> bool,
    ) -> Result<(), SandboxStoreError> {
        if stopping() {
            return Ok(());
        }
        let inventory = observe(
            "runtime_inventory",
            self.adapter.runtime_inventory(),
            |rows| Some(rows.len()),
        )
        .await
        .map_err(provider_error)?;
        if stopping() {
            return Ok(());
        }
        let authority = observe("durable_authority", self.authority(), |rows| {
            Some(rows.len())
        })
        .await?;
        let (limit, fallback) = self.startup_binding_schedule(&authority, &inventory, exclusive);
        let phase = Phase::new("binding_reconciliation");
        let report = batch::run(authority.values(), limit, stopping, |sandbox| {
            self.reconcile_startup_binding(sandbox, &inventory, now)
        })
        .await;
        let outcome = if report.error.is_some() {
            "error"
        } else {
            "ok"
        };
        tracing::info!(
            target: "cma_startup",
            parent: None,
            phase = "binding_reconciliation",
            configured_concurrency = BINDING_CONCURRENCY,
            effective_concurrency = limit,
            fallback,
            binding_count = authority.len(),
            started = report.started,
            settled = report.settled,
            deferred = report.deferred,
            active_peak = report.peak,
            outcome,
            "API startup binding batch settled"
        );
        phase.finish(outcome, Some(report.settled));
        if let Some(error) = report.error {
            return Err(error);
        }
        if stopping() {
            return Ok(());
        }
        if report.deferred > 0 {
            // Startup cleanup does not consult binding holds. Leave every
            // cleanup phase to the fresh, hold-aware background sweep when
            // even one binding remains unresolved; a conflict grants no
            // authority to reclaim its runtime, native Claim or home.
            tracing::warn!(target: "cma_startup", parent: None,
                deferred_bindings = report.deferred,
                "API startup cleanup deferred until held bindings reconcile");
            return Ok(());
        }
        observe(
            "orphan_reconciliation",
            async {
                for observation in &inventory {
                    if stopping() {
                        break;
                    }
                    self.reconcile_runtime(observation).await?;
                }
                Ok::<_, SandboxStoreError>(())
            },
            |_| Some(inventory.len()),
        )
        .await?;
        if stopping() {
            return Ok(());
        }
        observe(
            "home_reconciliation",
            self.reconcile_homes(stopping),
            |counts| Some(counts.0 + counts.1),
        )
        .await
        .map(|_| ())
    }
}
