use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use tokio::sync::{watch, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use super::capacity_scheduler::CapacityRecoverySweep;
use super::worker::SandboxWorker;
use super::workspace_worker::WorkspaceWorker;

const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(1);
const SANDBOX_CONCURRENT_SWEEPS: usize = 4;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

/// Periodically offers due durable steps; the store grants per-resource authority.
pub(crate) struct SandboxScheduler {
    worker: Arc<dyn SandboxSweep>,
    sweep_interval: Duration,
    concurrent_sweeps: usize,
    admitted: Arc<Notify>,
}

#[async_trait::async_trait]
trait SandboxSweep: Send + Sync {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError>;
}

#[async_trait::async_trait]
impl SandboxSweep for CapacityRecoverySweep {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        CapacityRecoverySweep::run_due(self, now)
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable(
                    "accepted capacity recovery discovery unavailable".into(),
                )
            })
    }
}

#[async_trait::async_trait]
impl SandboxSweep for SandboxWorker {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        let _priority = crate::background::shared().foreground();
        SandboxWorker::run_due(self, now).await
    }
}

#[async_trait::async_trait]
impl SandboxSweep for WorkspaceWorker {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        let _priority = crate::background::shared().foreground();
        WorkspaceWorker::run_due(self, now).await
    }
}

impl SandboxScheduler {
    pub(crate) fn for_capacity_recovery(worker: Arc<CapacityRecoverySweep>) -> Self {
        Self {
            worker,
            sweep_interval: Duration::from_secs(5),
            concurrent_sweeps: 1,
            admitted: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn new(worker: Arc<SandboxWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
            concurrent_sweeps: SANDBOX_CONCURRENT_SWEEPS,
            admitted: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn for_workspaces(worker: Arc<WorkspaceWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
            concurrent_sweeps: 1,
            admitted: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn with_admission_wake(mut self, admitted: Arc<Notify>) -> Self {
        self.admitted = admitted;
        self
    }

    #[cfg(test)]
    fn with_interval<W>(worker: Arc<W>, sweep_interval: Duration) -> Self
    where
        W: SandboxSweep + 'static,
    {
        assert!(!sweep_interval.is_zero());
        Self {
            worker,
            sweep_interval,
            concurrent_sweeps: 1,
            admitted: Arc::new(Notify::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        self.spawn_supervised(
            shutdown,
            crate::worker_supervision::Workers::new().get("sandboxes"),
        )
    }

    pub(crate) fn spawn_supervised(
        self,
        shutdown: watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> JoinHandle<()> {
        let owned = (self, JoinSet::new());
        crate::worker_supervision::spawn(
            owned,
            activity,
            shutdown,
            |(scheduler, sweeps), activity, mut shutdown| {
                Box::pin(async move {
                    let name = activity.snapshot().worker;
                    let telemetry = crate::telemetry::worker::Worker::start(name);
                    let poll_interval = scheduler.sweep_interval.min(Duration::from_secs(1));
                    let mut activity_tick = tokio::time::interval(poll_interval);
                    activity_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    let mut ticker = tokio::time::interval(scheduler.sweep_interval);
                    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        activity.waiting(
                            poll_interval,
                            if sweeps.len() >= scheduler.concurrent_sweeps {
                                "execution_slots"
                            } else {
                                "idle_scan"
                            },
                        );
                        telemetry.heartbeat();
                        telemetry.queue(0, sweeps.len());
                        if *shutdown.borrow() {
                            break;
                        }
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() {
                                    break;
                                }
                            }
                            result = sweeps.join_next(), if !sweeps.is_empty() => {
                                let outcome = match &result {
                                    Some(Ok(Ok(_))) => "ok",
                                    Some(Ok(Err(_))) => "error",
                                    _ => "terminated",
                                };
                                crate::telemetry::worker::completed(name, "execution", outcome);
                                match result {
                                    Some(Ok(Ok(completed))) if completed > 0 => {
                                        tracing::debug!(completed, "sandbox lifecycle work completed");
                                    }
                                    Some(Ok(Ok(_))) | None => {}
                                    Some(Ok(Err(error))) => {
                                        tracing::error!(error = %error, "sandbox lifecycle sweep failed");
                                    }
                                    Some(Err(error)) => {
                                        tracing::error!(error = %error, "sandbox lifecycle sweep terminated unexpectedly");
                                    }
                                }
                            }
                            _ = activity_tick.tick() => {}
                            _ = ticker.tick() => {
                                if sweeps.len() < scheduler.concurrent_sweeps {
                                    let worker = Arc::clone(&scheduler.worker);
                                    // A slow transfer must not prevent another resource (or a
                                    // newly fenced cancellation) from claiming its due work.
                                    sweeps.spawn(async move {
                                        layer2_core::diagnostic_context::DiagnosticContext::background("sandbox_scheduler").scope(worker.run_due(SystemTime::now())).await
                                    });
                                }
                            }
                            // Do not consume the retained hint while capacity is full.
                            _ = scheduler.admitted.notified(), if sweeps.len() < scheduler.concurrent_sweeps => {
                                let worker = Arc::clone(&scheduler.worker);
                                sweeps.spawn(async move {
                                    layer2_core::diagnostic_context::DiagnosticContext::background("sandbox_scheduler").scope(worker.run_due(SystemTime::now())).await
                                });
                            }
                        }
                        activity.scheduled();
                    }
                })
            },
            |(_, sweeps)| {
                Box::pin(async move {
                    let drain = async {
                        while let Some(result) = sweeps.join_next().await {
                            if let Ok(Err(error)) = result {
                                tracing::error!(%error, "sandbox lifecycle sweep failed during drain");
                            }
                        }
                    };
                    if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
                        tracing::warn!(pending = sweeps.len(), "sandbox scheduler drain timed out");
                        sweeps.shutdown().await;
                    }
                })
            },
        )
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
