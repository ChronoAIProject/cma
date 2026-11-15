//! The original ingress observes terminal custody even if a remote hop never
//! reaches it. Observation never replays content or changes durable receipts.
use crate::{
    delivery::{custom::CustomDelivery, local_stream::LocalStreamOutcome},
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    turns::routed::{journal, model::delivery_event, ExecutionError, Journal, TargetState},
};
use cmaeg_core::seal::ReplyAddress;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

const POLL: Duration = Duration::from_secs(1);
const READ_BUDGET: Duration = Duration::from_secs(2);

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod tests;

#[derive(Clone)]
pub(crate) struct StreamMonitor {
    journal: Journal,
    custom: CustomDelivery,
    reads: Arc<Semaphore>,
}

impl StreamMonitor {
    pub(crate) fn new(journal: Journal, custom: CustomDelivery) -> Self {
        Self {
            journal,
            custom,
            reads: Arc::new(Semaphore::new(32)),
        }
    }

    pub(crate) fn start(&self, plan: &AdmittedPlan) {
        let Some(target) = plan.targets.first() else {
            return;
        };
        let event = delivery_event(plan, target);
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return;
        };
        if !self.custom.waiters.monitor(waiter) {
            return;
        }
        let waiter = waiter.clone();
        let monitor = self.clone();
        let mut plan = plan.clone();
        // Observation needs the binding fields, not retained ingress bytes or
        // any secondary target. Do not keep request bodies for stream lifetime.
        plan.source_body = Vec::new();
        plan.targets.truncate(1);
        let target = target.clone();
        tokio::spawn(async move {
            while monitor.custom.waiters.contains(&waiter) {
                let observation =
                    tokio::time::timeout(READ_BUDGET, monitor.observe(&plan, &target)).await;
                let detail = match observation {
                    Ok(Ok(false)) => {
                        tokio::time::sleep(POLL).await;
                        continue;
                    }
                    Ok(Ok(true)) => "sync_delivery_lost",
                    // A failed observation is uncertainty, not proof of loss.
                    // Close this connection without changing the provider work.
                    _ => "sync_delivery_observation_unavailable",
                };
                monitor.custom.waiters.emit(
                    &waiter,
                    "cmaeg.failed",
                    &json!({"event_id":event.event_id,"conversation_id":event.conversation_id,
                        "error":{"code":"internal","detail":detail}}),
                    true,
                );
                break;
            }
        });
    }

    async fn observe(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<bool, ExecutionError> {
        let _permit = self
            .reads
            .acquire()
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        if plan.retained_until <= (self.journal.clock)() {
            return Err(ExecutionError::Expired);
        }
        let Some((state, _)) = self.journal.read::<TargetState>(&target.id).await? else {
            // An admitted target may not have been claimed yet. There is no
            // short total deadline for valid queued or executing work.
            return Ok(false);
        };
        journal::validate(&state, plan, target)?;
        Ok(state.phase.terminal()
            || state.delivery.as_ref().is_some_and(|checkpoint| {
                checkpoint.terminal
                    && matches!(
                        checkpoint.custom.as_ref().and_then(|custom| custom.local),
                        Some(LocalStreamOutcome::Lost | LocalStreamOutcome::Unavailable)
                    )
            }))
    }
}
