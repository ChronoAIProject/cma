//! Repair delayed intent admission before discovering the live FIFO frontier.
use super::{config::STORE_TIMEOUT, effects::EffectExecutor};
use crate::{
    ingress::routing_store::RecoveryPage,
    replicas::diagnostics::{LogLimit, ScanError, Stage},
    turns::routed::scheduler::RecoverySource,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct Recovery {
    frontier: Arc<dyn RecoverySource>,
    effects: Arc<dyn EffectExecutor>,
    cursor: Mutex<Option<String>>,
    logs: std::sync::Mutex<LogLimit>,
}

impl Recovery {
    pub fn new(frontier: Arc<dyn RecoverySource>, effects: Arc<dyn EffectExecutor>) -> Self {
        Self {
            frontier,
            effects,
            cursor: Mutex::new(None),
            logs: Default::default(),
        }
    }
}

impl RecoverySource for Recovery {
    fn start_measurement(&self) -> Option<crate::telemetry::provider_start::Measurement> {
        self.frontier.start_measurement()
    }

    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, ScanError>> {
        Box::pin(async move {
            // Effect roots and partition heads have different index domains.
            // Preserve this cursor on failure, without starving queued work.
            let mut cursor = self.cursor.lock().await;
            let result =
                tokio::time::timeout(STORE_TIMEOUT, self.effects.recover(cursor.as_deref())).await;
            let outcome = match result {
                Ok(Ok(next)) => {
                    *cursor = next;
                    "repaired"
                }
                Ok(Err(error)) => {
                    ScanError::new(
                        super::unresolved::execution_error(error),
                        Stage::Effect,
                        None,
                    )
                    .report_scan(&mut self.logs.lock().unwrap_or_else(|e| e.into_inner()));
                    "unavailable"
                }
                Err(_) => {
                    ScanError::new(super::Error::Unavailable, Stage::Effect, None)
                        .report_scan(&mut self.logs.lock().unwrap_or_else(|e| e.into_inner()));
                    "deadline"
                }
            };
            metrics::counter!("cmaeg_replica_effect_recovery_total", "outcome" => outcome)
                .increment(1);
            drop(cursor);
            self.frontier.page(after).await
        })
    }
}

#[cfg(test)]
#[path = "effect_recovery_tests.rs"]
mod tests;
