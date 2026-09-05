use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::agent_worker::AgentWorker;
use super::worker::SandboxWorker;

const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(1);

/// Periodically offers due durable steps to one fenced worker instance.
pub(crate) struct SandboxScheduler {
    worker: Arc<dyn SandboxSweep>,
    sweep_interval: Duration,
}

#[async_trait::async_trait]
trait SandboxSweep: Send + Sync {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError>;
}

#[async_trait::async_trait]
impl SandboxSweep for SandboxWorker {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        SandboxWorker::run_due(self, now).await
    }
}

#[async_trait::async_trait]
impl SandboxSweep for AgentWorker {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        AgentWorker::run_due(self, now).await
    }
}

impl SandboxScheduler {
    pub(crate) fn new(worker: Arc<SandboxWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
        }
    }

    pub(crate) fn for_agents(worker: Arc<AgentWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
        }
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
        }
    }

    pub(crate) fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.sweep_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                match self.worker.run_due(SystemTime::now()).await {
                    Ok(completed) if completed > 0 => {
                        tracing::debug!(completed, "sandbox lifecycle work completed");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(error = %error, "sandbox lifecycle sweep failed");
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;

    #[derive(Default)]
    struct SweepProbe {
        calls: AtomicUsize,
        called: Notify,
    }

    #[async_trait::async_trait]
    impl SandboxSweep for SweepProbe {
        async fn run_due(&self, _now: SystemTime) -> Result<usize, SandboxStoreError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.called.notify_one();
            if call == 0 {
                Err(SandboxStoreError::Unavailable(
                    "simulated scheduler dependency failure".into(),
                ))
            } else {
                Ok(1)
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn scheduler_keeps_offering_periodic_work_after_a_failed_sweep() {
        let probe = Arc::new(SweepProbe::default());
        let first = probe.called.notified();
        let scheduler =
            SandboxScheduler::with_interval(Arc::clone(&probe), Duration::from_secs(1)).spawn();
        first.await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);

        let second = probe.called.notified();
        tokio::time::advance(Duration::from_secs(1)).await;
        second.await;
        assert!(probe.calls.load(Ordering::SeqCst) >= 2);
        assert!(!scheduler.is_finished());
        scheduler.abort();
    }
}
