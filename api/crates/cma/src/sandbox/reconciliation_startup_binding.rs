//! Revision conflicts defer one binding, without weakening its authority fences.
use super::super::{corrupt, SandboxReconciler};
use layer2_core::sandbox::{Sandbox, SandboxStoreError};
use layer3_adapter::RuntimeObservation;
use std::time::SystemTime;

const ATTEMPTS: usize = 3;

impl SandboxReconciler {
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
            let result = async {
                let fresh;
                let (current, now) = if attempt == 1 {
                    (sandbox, floor)
                } else {
                    let Some(current) = self.store.get_sandbox(&sandbox.id).await? else {
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
                    fresh = current;
                    // A concurrent writer may have advanced updated_at since
                    // startup began. Keep an explicit future floor intact; do
                    // not move any provider-observation boundary forward.
                    (&fresh, SystemTime::now().max(floor))
                };
                self.reconcile_binding(current, inventory, now).await
            }
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
                Err(error) => return Err(error),
            }
        }
        hold.raced();
        Ok(false)
    }
}
