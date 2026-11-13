//! Keep runtime-recovery scheduling observable while one owned batch runs.
use super::SandboxReconciler;
use std::{sync::Arc, time::Duration, time::SystemTime};
use tokio::{sync::watch, task::JoinHandle, task::JoinSet, time::MissedTickBehavior};

const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(15);
const SCHEDULING_INTERVAL: Duration = Duration::from_secs(1);

impl SandboxReconciler {
    pub(crate) fn spawn_supervised(
        self,
        shutdown: watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> JoinHandle<()> {
        crate::worker_supervision::spawn(
            (Arc::new(self), JoinSet::new()),
            activity,
            shutdown,
            |(reconciler, tasks), activity, mut shutdown| {
                Box::pin(async move {
                    let telemetry =
                        crate::telemetry::worker::Worker::start("sandbox_reconciliation");
                    let mut scheduling = tokio::time::interval(SCHEDULING_INTERVAL);
                    let mut batches = tokio::time::interval(RECONCILIATION_INTERVAL);
                    scheduling.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    batches.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        activity.waiting(
                            SCHEDULING_INTERVAL,
                            if tasks.is_empty() {
                                "idle_scan"
                            } else {
                                "reconciliation"
                            },
                        );
                        telemetry.heartbeat();
                        telemetry.queue(0, tasks.len());
                        if *shutdown.borrow() {
                            break;
                        }
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() { break; }
                            }
                            result = tasks.join_next(), if !tasks.is_empty() => {
                                let outcome = match result {
                                    Some(Ok(Ok(_))) => "ok",
                                    Some(Ok(Err(error))) => {
                                        tracing::error!(%error, "sandbox runtime reconciliation failed");
                                        "error"
                                    }
                                    Some(Err(error)) => {
                                        tracing::error!(%error, "sandbox runtime reconciliation child terminated");
                                        "terminated"
                                    }
                                    None => continue,
                                };
                                crate::telemetry::worker::completed("sandbox_reconciliation", "reconciliation", outcome);
                            }
                            _ = scheduling.tick() => {}
                            _ = async {
                                if reconciler.resume_background {
                                    tokio::time::sleep_until(reconciler.background_work.next_run_at()).await;
                                } else {
                                    batches.tick().await;
                                }
                            }, if !reconciler.resume_background || tasks.is_empty() => {
                                if tasks.is_empty() {
                                    let reconciler = Arc::clone(reconciler);
                                    let activity = activity.clone();
                                    tasks.spawn(async move {
                                        let _phase = activity.reconciliation();
                                        // The owned child runs outside the cancellable wait.
                                        // Busy/yield/backoff timing comes from BackgroundWork;
                                        // shutdown still drains its bounded durable operation.
                                        reconciler.background_work.run_resumable(|| reconciler.run_due(SystemTime::now())).await
                                    });
                                }
                            }
                        }
                        activity.scheduled();
                    }
                })
            },
            |(_, tasks)| {
                Box::pin(async move {
                    // The existing coordinator bounds total shutdown. Scheduling
                    // recovery never cancels an effectful reconciliation batch.
                    while tasks.join_next().await.is_some() {}
                })
            },
        )
    }
}
