use super::*;
use crate::worker_supervision::Activity;
use tokio::{sync::watch, task::JoinHandle};

const NAME: &str = "resource_revocations";

impl RevocationSweep {
    /// Composition supplies a dedicated activity only after policy readiness.
    pub(crate) fn spawn_supervised(
        mut self,
        shutdown: watch::Receiver<bool>,
        activity: Activity,
    ) -> JoinHandle<()> {
        self.activity = Some(activity.clone());
        crate::worker_supervision::spawn(
            self,
            activity,
            shutdown,
            |sweep, activity, shutdown| {
                Box::pin(async move {
                    if sweep.telemetry.is_none() {
                        sweep.telemetry = Some(crate::telemetry::worker::Worker::start(NAME));
                    }
                    loop {
                        if *shutdown.borrow() || shutdown.has_changed().is_err() {
                            break;
                        }
                        activity.waiting(
                            SCAN_INTERVAL,
                            if sweep.in_flight.len() >= OPERATION_CONCURRENCY {
                                "execution_slots"
                            } else if sweep.discovering {
                                "discovery"
                            } else {
                                "idle_scan"
                            },
                        );
                        sweep.telemetry.as_ref().unwrap().heartbeat();
                        sweep.observe_queue();
                        let event = tokio::select! {
                            biased;
                            _ = crate::shutdown::signalled(shutdown.clone()) => break,
                            event = sweep.next() => event,
                        };
                        activity.scheduled();
                        match event {
                            SweepEvent::Discovered { count, .. } => {
                                crate::telemetry::worker::completed(
                                    NAME,
                                    "discovery",
                                    if count == 0 { "idle" } else { "ok" },
                                );
                            }
                            SweepEvent::DiscoveryFailed(error) => {
                                crate::telemetry::worker::completed(NAME, "discovery", "error");
                                tracing::warn!(?error, "Revocation discovery remains incomplete");
                            }
                            SweepEvent::Reconciled { operation, result } => {
                                report(&operation, &result)
                            }
                            SweepEvent::Waiting => {}
                            SweepEvent::SchedulingStopped => break,
                        }
                    }
                })
            },
            |sweep| {
                Box::pin(async move {
                    for (operation, result) in sweep.shutdown().await {
                        report(&operation, &result);
                    }
                })
            },
        )
    }
}

fn report(operation: &wire::Digest, result: &ExecutionResult<Progress>) {
    let outcome = match result {
        Ok(Progress::Effective) => "ok",
        Ok(Progress::Pending {
            completed,
            total,
            deadline_exceeded,
            failures,
        }) => {
            tracing::info!(
                operation = %operation.as_str(), completed, total, deadline_exceeded,
                failures = failures.len(), "Revocation remains pending"
            );
            "pending"
        }
        Err(error) => {
            tracing::warn!(operation = %operation.as_str(), ?error, "Revocation reconciliation incomplete");
            if *error == ExecutionError::Unknown {
                "unknown"
            } else {
                "error"
            }
        }
    };
    crate::telemetry::worker::completed(NAME, "reconciliation", outcome);
}
