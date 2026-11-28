//! Retryable startup failures defer one binding while its authority stays fenced.
use super::super::{corrupt, diagnostics, provider_error, transient, SandboxReconciler};
use layer2_core::sandbox::store::ReconciliationSandbox;
use layer2_core::sandbox::{Sandbox, SandboxStoreError};
use layer3_adapter::binding_diagnostics::{measure, Stage};
use layer3_adapter::RuntimeObservation;
use sha2::{Digest, Sha256};
use std::time::SystemTime;

const ATTEMPTS: usize = 3;

impl SandboxReconciler {
    pub(crate) fn with_startup_native_home_hold(mut self, enabled: bool) -> Self {
        self.startup_native_home_hold_enabled = enabled;
        self
    }

    pub(super) async fn reconcile_startup_binding(
        &self,
        sandbox: &Sandbox,
        inventory: &[RuntimeObservation],
        floor: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let Some(hold) = self.binding_holds.begin(sandbox, inventory) else {
            return Ok(false);
        };
        for attempt in 1..=ATTEMPTS {
            let (result, failure) = transient::classify(async {
                let fresh;
                let (current, now) = if attempt == 1 {
                    // Inventory and authority reads may outlive the startup
                    // timestamp while another writer advances this parent.
                    (sandbox, SystemTime::now().max(floor))
                } else {
                    let ReconciliationSandbox::Present(current) =
                        self.store.reconciliation_sandbox(&sandbox.id).await?
                    else {
                        return Ok(true);
                    };
                    hold.observe(&current, inventory);
                    current
                        .validate()
                        .map_err(|error| corrupt(&error.to_string()))?;
                    // Parallel admission proved disjoint targets only for the
                    // original snapshot. Keep changed aliases protected for
                    // the background sweep, after this batch has drained.
                    if current.runtime != sandbox.runtime
                        || current.lifecycle.suspended_runtime
                            != sandbox.lifecycle.suspended_runtime
                        || current.lifecycle.home != sandbox.lifecycle.home
                    {
                        return Ok(false);
                    }
                    fresh = *current;
                    // A concurrent writer may have advanced updated_at since
                    // startup began. Keep an explicit future floor intact; do
                    // not move any provider-observation boundary forward.
                    (&fresh, SystemTime::now().max(floor))
                };
                self.reconcile_binding(current, inventory, now).await
            })
            .await;
            match result {
                Ok(true) => {
                    hold.complete();
                    return Ok(true);
                }
                Ok(false) => break,
                Err(SandboxStoreError::Conflict) => {
                    tracing::warn!(target: "cma_startup", parent: None,
                        attempt, attempt_limit = ATTEMPTS,
                        retry = attempt < ATTEMPTS,
                        "Sandbox revision conflict during startup binding reconciliation");
                }
                Err(SandboxStoreError::Unavailable(_))
                    if failure == Some(transient::StartupFailure::Unavailable) =>
                {
                    // One unreachable runtime must not stop the process. The
                    // hold keeps its runtime, Claim and home protected, and a
                    // deferred binding already skips every startup cleanup.
                    // Hashed like other lifecycle diagnostics, so an operator can
                    // find a wedged runtime without logging its identifier.
                    let sandbox_hash =
                        format!("{:x}", Sha256::digest(sandbox.id.as_str().as_bytes()));
                    tracing::warn!(target: "cma_startup", parent: None, attempt, sandbox_hash,
                        "Sandbox runtime unavailable during startup binding; deferred to background reconciliation");
                    hold.unavailable();
                    return Ok(false);
                }
                Err(SandboxStoreError::Unavailable(_))
                    if matches!(
                        failure,
                        Some(
                            transient::StartupFailure::NativePoolConfigurationDrift
                                | transient::StartupFailure::AdmittedSandboxSpecDrift
                        )
                    ) || (failure
                        == Some(transient::StartupFailure::NativePoolDefinitionAbsent)
                        && sandbox.runtime.is_some()
                        && sandbox
                            .lifecycle
                            .home
                            .as_ref()
                            .and_then(|home| home.native_pool.as_ref())
                            .is_some_and(|pool| pool.claim.is_some()))
                        || (self.startup_native_home_hold_enabled
                            && failure == Some(transient::StartupFailure::NativeHomeIdentity)) =>
                {
                    let sandbox_hash =
                        format!("{:x}", Sha256::digest(sandbox.id.as_str().as_bytes()));
                    let reason = match failure {
                        Some(transient::StartupFailure::NativePoolConfigurationDrift) => {
                            "native_pool_configuration_drift"
                        }
                        Some(transient::StartupFailure::AdmittedSandboxSpecDrift) => {
                            "admitted_sandbox_spec_drift"
                        }
                        Some(transient::StartupFailure::NativePoolDefinitionAbsent) => {
                            "native_pool_definition_absent"
                        }
                        _ => "native_home_identity_refused",
                    };
                    tracing::warn!(target: "cma_startup", parent: None, attempt, sandbox_hash, reason,
                        "Sandbox native binding deferred to background reconciliation");
                    hold.refused();
                    return Ok(false);
                }
                Err(error) => {
                    if matches!(error, SandboxStoreError::Corrupt(_))
                        && matches!(
                            self.store.reconciliation_sandbox(&sandbox.id).await?,
                            ReconciliationSandbox::Quarantined
                        )
                    {
                        hold.complete();
                        return Ok(true);
                    }
                    return Err(error);
                }
            }
        }
        hold.raced();
        Ok(false)
    }

    pub(in crate::sandbox::reconciliation) async fn binding_result<T>(
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
                self.store.reconciliation_sandbox(&sandbox.id),
                diagnostics::store_outcome,
            )
            .await?;
            match current {
                ReconciliationSandbox::Quarantined => return Err(SandboxStoreError::Conflict),
                ReconciliationSandbox::Present(row)
                    if row.revision > sandbox.revision
                        && (row.runtime != sandbox.runtime
                            || row.lifecycle.suspended_runtime
                                != sandbox.lifecycle.suspended_runtime
                            || row.lifecycle.home != sandbox.lifecycle.home) =>
                {
                    return Err(SandboxStoreError::Conflict);
                }
                _ => {}
            }
            return Err(transient::native_home_identity(error));
        }
        Err(provider_error(error))
    }
}
