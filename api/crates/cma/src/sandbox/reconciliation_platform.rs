//! The platform spend phase of the reconciliation sweep (contract §5.3): every
//! replica re-evaluates time-driven transitions each sweep, and a spend stop
//! drives the platform pause of quiet Sandboxes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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

/// One evaluation per interval of the sweep clock, on every `run_due`, so a
/// stalled sweep phase cannot hold back a stop, a lift or staleness.
const EVALUATION_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Default)]
pub(super) struct PlatformSpendSchedule(Mutex<Option<(SystemTime, bool)>>);

impl SandboxReconciler {
    pub(crate) fn with_platform_spend(mut self, sweep: Arc<dyn PlatformSpendSweep>) -> Self {
        self.platform_spend = Some(sweep);
        self
    }

    /// Whether `SpendStop` is in force, re-evaluated at most once per
    /// interval. Never fails the sweep: an evaluation error keeps the last
    /// known answer and retries on the next call; admission fails closed on
    /// its own.
    pub(super) async fn platform_spend_stop(&self, now: SystemTime) -> bool {
        let Some(sweep) = &self.platform_spend else {
            return false;
        };
        let last = *self
            .platform_schedule
            .0
            .lock()
            .expect("platform spend schedule poisoned");
        if let Some((at, stop)) = last {
            if now
                .duration_since(at)
                .is_ok_and(|elapsed| elapsed < EVALUATION_INTERVAL)
            {
                return stop;
            }
        }
        let seconds = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        match sweep.evaluate_platform_spend(seconds).await {
            Ok(stop) => {
                let stop = stop.unwrap_or(false);
                *self
                    .platform_schedule
                    .0
                    .lock()
                    .expect("platform spend schedule poisoned") = Some((now, stop));
                stop
            }
            Err(error) => {
                tracing::warn!(parent: None, phase = "platform_spend", error = %error,
                    "platform spend evaluation failed");
                last.is_some_and(|(_, stop)| stop)
            }
        }
    }
}
