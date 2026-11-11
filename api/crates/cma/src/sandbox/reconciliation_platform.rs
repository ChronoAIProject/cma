//! The platform spend phase of the reconciliation sweep (contract §5.3): every
//! replica re-evaluates time-driven transitions each sweep, and a spend stop
//! drives the platform pause of quiet Sandboxes.

use std::sync::Arc;
use std::time::SystemTime;

use layer2_core::budget::{BudgetError, ClosureReason, PlatformSpendLedger};

use super::SandboxReconciler;
use crate::store::MongoSandboxStore;

/// Evaluate the current period and report whether `SpendStop` is in force;
/// `None` without an enabled platform policy.
#[async_trait::async_trait]
pub(crate) trait PlatformSpendSweep: Send + Sync {
    async fn evaluate_platform_spend(&self, now: u64) -> Result<Option<bool>, BudgetError>;
}

#[async_trait::async_trait]
impl PlatformSpendSweep for MongoSandboxStore {
    async fn evaluate_platform_spend(&self, now: u64) -> Result<Option<bool>, BudgetError> {
        if self.evaluate_period(now).await?.is_none() {
            return Ok(None);
        }
        let state = self.admission_state(now).await?;
        Ok(Some(
            state
                .closures
                .iter()
                .any(|closure| closure.reason == ClosureReason::SpendStop),
        ))
    }
}

impl SandboxReconciler {
    pub(crate) fn with_platform_spend(mut self, sweep: Arc<dyn PlatformSpendSweep>) -> Self {
        self.platform_spend = Some(sweep);
        self
    }

    /// Never fails the sweep: an unreadable period leaves pausing to the
    /// inactivity policy, and admission fails closed on its own.
    pub(super) async fn platform_spend_stop(&self, now: SystemTime) -> bool {
        let Some(sweep) = &self.platform_spend else {
            return false;
        };
        let seconds = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        match sweep.evaluate_platform_spend(seconds).await {
            Ok(stop) => stop.unwrap_or(false),
            Err(error) => {
                tracing::warn!(parent: None, phase = "platform_spend", error = %error,
                    "platform spend evaluation failed");
                false
            }
        }
    }
}
