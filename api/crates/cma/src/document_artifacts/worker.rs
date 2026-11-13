//! Bounded document maintenance outside request admission and startup readiness.
use layer2_core::{
    document_artifacts::{DocumentMaintenance, ReclaimOutcome, ORPHAN_ALERT_AGE, RECLAIM_INTERVAL},
    sandbox::store::SandboxStoreError,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};

pub(crate) struct DocumentMaintenanceWorker {
    maintenance: Option<DocumentMaintenance>,
    budget: Arc<crate::background::BackgroundBudget>,
}
impl Default for DocumentMaintenanceWorker {
    fn default() -> Self {
        Self::new(None)
    }
}
impl DocumentMaintenanceWorker {
    pub(crate) fn new(maintenance: Option<DocumentMaintenance>) -> Self {
        Self {
            maintenance,
            budget: crate::background::shared(),
        }
    }
    #[cfg(test)]
    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        self.spawn_supervised(
            shutdown,
            crate::worker_supervision::Workers::new().get("document_maintenance"),
        )
    }
    pub(crate) fn spawn_supervised(
        self,
        shutdown: watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> JoinHandle<()> {
        crate::worker_supervision::spawn(
            (Arc::new(self), JoinSet::new()),
            activity,
            shutdown,
            |(worker, tasks), activity, mut shutdown| {
                Box::pin(async move {
                    let telemetry = crate::telemetry::worker::Worker::start("document_maintenance");
                    let mut scheduling = tokio::time::interval(Duration::from_secs(1));
                    scheduling.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    let enabled = worker.maintenance.is_some();
                    if !enabled {
                        record_blocked(&mut None, "reclaim_disabled_or_storage_unconfigured");
                    }
                    loop {
                        if *shutdown.borrow() {
                            break;
                        }
                        if enabled && tasks.is_empty() {
                            let worker = Arc::clone(worker);
                            let mut signal = shutdown.clone();
                            // The complete paced loop remains owned across scheduler
                            // recovery, including any ambiguous reclaim effects.
                            tasks.spawn(async move {
                                tokio::select! {
                                    biased;
                                    _ = crate::shutdown::signalled(signal.clone()) => (),
                                    _ = worker.run(&mut signal) => (),
                                }
                            });
                        }
                        activity.waiting(
                            Duration::from_secs(1),
                            if enabled { "maintenance" } else { "disabled" },
                        );
                        telemetry.heartbeat();
                        telemetry.queue(0, tasks.len());
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() { break; }
                            }
                            result = tasks.join_next(), if !tasks.is_empty() => {
                                if let Some(Err(error)) = result {
                                    tracing::error!(%error, "Document maintenance child terminated");
                                }
                                return;
                            }
                            _ = scheduling.tick() => {},
                        }
                        activity.scheduled();
                    }
                })
            },
            |(_, tasks)| Box::pin(async move { while tasks.join_next().await.is_some() {} }),
        )
    }
    async fn run(&self, shutdown: &mut watch::Receiver<bool>) {
        let Some(maintenance) = &self.maintenance else {
            record_blocked(&mut None, "reclaim_disabled_or_storage_unconfigured");
            return;
        };
        // Preserve startup and reconnect capacity for foreground work.
        tokio::time::sleep(Duration::from_secs(30)).await;
        let reclaim = crate::background::BackgroundWork::new(
            self.budget.clone(),
            "document_reclaim",
            Duration::from_secs(30),
            RECLAIM_INTERVAL,
        );
        let mut blocked_reason = None;
        let mut alerted = false;
        loop {
            if *shutdown.borrow() {
                return;
            }
            // Future purge steps share this worker and budget, ahead of reclaim.
            let result = reclaim
                .run(|| async {
                    maintenance
                        .reclaim_step(mongodb::bson::DateTime::now().timestamp_millis())
                        .await
                        .map_err(|_| {
                            SandboxStoreError::Unavailable("document reclaim unavailable".into())
                        })
                })
                .await;
            let mut retry_after = None;
            match result {
                Ok(Some(outcome)) => {
                    emit(&outcome);
                    retry_after = outcome.retry_after;
                    if outcome.blocked {
                        record_blocked(
                            &mut blocked_reason,
                            outcome.blocked_reason.unwrap_or("reclaim_blocked"),
                        );
                    } else {
                        blocked_reason = None;
                        let old = outcome.backlog.oldest_age_seconds >= ORPHAN_ALERT_AGE.as_secs();
                        if old && !alerted {
                            tracing::warn!(
                                age_seconds = outcome.backlog.oldest_age_seconds,
                                pending_objects = outcome.backlog.pending_objects,
                                "Document orphan age exceeds the alert threshold"
                            );
                        }
                        alerted = old;
                    }
                }
                Ok(None) => record_blocked(&mut blocked_reason, "background_budget_denied"),
                Err(_) => record_blocked(&mut blocked_reason, "reclaim_unavailable"),
            }
            // Denied work is never queued. A dependency's longer minimum delay
            // also applies after BackgroundWork's own jittered pacing.
            tokio::time::sleep(next_delay(retry_after)).await;
        }
    }
}
fn next_delay(retry: Option<Duration>) -> Duration {
    retry.unwrap_or_default().max(RECLAIM_INTERVAL)
}
fn record_blocked(previous: &mut Option<&'static str>, reason: &'static str) {
    metrics::gauge!("cma_document_reclaim_blocked").set(1);
    if *previous != Some(reason) {
        tracing::info!(reason, "Document reclaim blocked");
        *previous = Some(reason);
    }
}
fn emit(outcome: &ReclaimOutcome) {
    metrics::gauge!("cma_document_reclaim_blocked").set(u8::from(outcome.blocked));
    // An unavailable/blocked ledger supplies no fresh backlog evidence.
    if !outcome.blocked {
        metrics::gauge!("cma_document_reclaim_pending_objects")
            .set(outcome.backlog.pending_objects as f64);
        metrics::gauge!("cma_document_orphan_oldest_age_seconds")
            .set(outcome.backlog.oldest_age_seconds as f64);
    }
    for (label, count) in [
        ("committed", outcome.committed),
        ("condemned", outcome.condemned),
        ("released", outcome.released),
        ("delete_failed", outcome.delete_failed),
    ] {
        metrics::counter!("cma_document_reclaim_objects_total", "outcome" => label)
            .increment(count);
    }
}
pub(crate) fn describe_metrics() {
    metrics::describe_gauge!(
        "cma_document_reclaim_pending_objects",
        "Stale reserved and condemned document objects at the last successful background sample"
    );
    metrics::describe_gauge!(
        "cma_document_orphan_oldest_age_seconds",
        "Oldest pending document object age at the last successful background sample"
    );
    metrics::describe_gauge!(
        "cma_document_reclaim_blocked",
        "One when document reclaim cannot progress, zero after an unblocked round"
    );
    metrics::describe_counter!(
        "cma_document_reclaim_objects_total",
        "Observed document reclaim transitions by outcome; not durable exactly-once accounting"
    );
}
#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
