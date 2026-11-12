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

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut sweeps = JoinSet::new();
            let mut ticker = tokio::time::interval(self.sweep_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
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
                    _ = ticker.tick(), if sweeps.len() < self.concurrent_sweeps => {
                        let worker = Arc::clone(&self.worker);
                        // A slow transfer must not prevent another resource (or a
                        // newly fenced cancellation) from claiming its due work.
                        sweeps.spawn(async move {
                            layer2_core::diagnostic_context::DiagnosticContext::background("sandbox_scheduler").scope(worker.run_due(SystemTime::now())).await
                        });
                    }
                    // Do not consume the retained hint while capacity is full.
                    _ = self.admitted.notified(), if sweeps.len() < self.concurrent_sweeps => {
                        let worker = Arc::clone(&self.worker);
                        sweeps.spawn(async move {
                            layer2_core::diagnostic_context::DiagnosticContext::background("sandbox_scheduler").scope(worker.run_due(SystemTime::now())).await
                        });
                    }
                }
            }
            if tokio::time::timeout(DRAIN_TIMEOUT, async {
                while let Some(result) = sweeps.join_next().await {
                    if let Ok(result) = result {
                        match result {
                            Ok(completed) if completed > 0 => {
                                tracing::debug!(completed, "sandbox lifecycle work completed");
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::error!(error = %error, "sandbox lifecycle sweep failed");
                            }
                        }
                    }
                }
            })
            .await
            .is_err()
            {
                tracing::warn!(pending = sweeps.len(), "sandbox scheduler drain timed out");
                sweeps.shutdown().await;
            }
        })
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
