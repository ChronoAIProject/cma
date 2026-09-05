use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::worker::{LaunchWorker, LaunchWorkerError};

const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct LaunchScheduler {
    worker: Arc<dyn LaunchSweep>,
    sweep_interval: Duration,
}

#[async_trait::async_trait]
trait LaunchSweep: Send + Sync {
    async fn run_due(&self, now: SystemTime) -> Result<usize, LaunchWorkerError>;
}

#[async_trait::async_trait]
impl LaunchSweep for LaunchWorker {
    async fn run_due(&self, now: SystemTime) -> Result<usize, LaunchWorkerError> {
        LaunchWorker::run_due(self, now).await
    }
}

impl LaunchScheduler {
    pub(crate) fn new(worker: Arc<LaunchWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
        }
    }

    #[cfg(test)]
    fn with_interval<W>(worker: Arc<W>, sweep_interval: Duration) -> Self
    where
        W: LaunchSweep + 'static,
    {
        assert!(!sweep_interval.is_zero());
        Self {
            worker,
            sweep_interval,
        }
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
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
                    _ = ticker.tick() => {
                        match self.worker.run_due(SystemTime::now()).await {
                            Ok(completed) if completed > 0 => {
                                tracing::debug!(completed, "Trigger Launch work completed");
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::error!(error = %error, "Trigger Launch sweep failed");
                            }
                        }
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
    impl LaunchSweep for SweepProbe {
        async fn run_due(&self, _now: SystemTime) -> Result<usize, LaunchWorkerError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.called.notify_one();
            if call == 0 {
                Err(LaunchWorkerError::Corrupt(
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
        let (_shutdown, receiver) = watch::channel(false);
        let scheduler = LaunchScheduler::with_interval(Arc::clone(&probe), Duration::from_secs(1))
            .spawn(receiver);
        first.await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);

        let second = probe.called.notified();
        tokio::time::advance(Duration::from_secs(1)).await;
        second.await;
        assert!(probe.calls.load(Ordering::SeqCst) >= 2);
        assert!(!scheduler.is_finished());
        scheduler.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn scheduler_exits_after_shutdown_between_sweeps() {
        let probe = Arc::new(SweepProbe::default());
        let first = probe.called.notified();
        let (shutdown, receiver) = watch::channel(false);
        let scheduler = LaunchScheduler::with_interval(Arc::clone(&probe), Duration::from_secs(1))
            .spawn(receiver);
        first.await;

        shutdown.send(true).unwrap();
        scheduler.await.unwrap();
    }
}
