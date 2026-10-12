use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

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
impl SandboxSweep for WorkspaceWorker {
    async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        WorkspaceWorker::run_due(self, now).await
    }
}

impl SandboxScheduler {
    pub(crate) fn new(worker: Arc<SandboxWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
            concurrent_sweeps: SANDBOX_CONCURRENT_SWEEPS,
        }
    }

    pub(crate) fn for_workspaces(worker: Arc<WorkspaceWorker>) -> Self {
        Self {
            worker,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
            concurrent_sweeps: 1,
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
            concurrent_sweeps: 1,
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
                            worker.run_due(SystemTime::now()).await
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
        let (_shutdown, receiver) = watch::channel(false);
        let scheduler = SandboxScheduler::with_interval(Arc::clone(&probe), Duration::from_secs(1))
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
        let scheduler = SandboxScheduler::with_interval(Arc::clone(&probe), Duration::from_secs(1))
            .spawn(receiver);
        first.await;

        shutdown.send(true).unwrap();
        scheduler.await.unwrap();
    }

    struct BlockingProbe {
        calls: AtomicUsize,
        active: Arc<AtomicUsize>,
        called: Notify,
        release: Notify,
        block_all: bool,
    }

    struct ActiveCall(Arc<AtomicUsize>);

    impl Drop for ActiveCall {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl SandboxSweep for BlockingProbe {
        async fn run_due(&self, _now: SystemTime) -> Result<usize, SandboxStoreError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveCall(Arc::clone(&self.active));
            self.called.notify_one();
            if call == 0 || self.block_all {
                self.release.notified().await;
            }
            Ok(1)
        }
    }

    fn blocking_probe(block_all: bool) -> Arc<BlockingProbe> {
        Arc::new(BlockingProbe {
            calls: AtomicUsize::new(0),
            active: Arc::new(AtomicUsize::new(0)),
            called: Notify::new(),
            release: Notify::new(),
            block_all,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn slow_sandbox_work_does_not_block_new_due_work_or_pause_cancellation() {
        let probe = blocking_probe(false);
        let first = probe.called.notified();
        let (shutdown, receiver) = watch::channel(false);
        let mut scheduler =
            SandboxScheduler::with_interval(Arc::clone(&probe), DEFAULT_SWEEP_INTERVAL);
        scheduler.concurrent_sweeps = SANDBOX_CONCURRENT_SWEEPS;
        let scheduler = scheduler.spawn(receiver);
        first.await;
        for expected in 2..=6 {
            let next = probe.called.notified();
            tokio::time::advance(DEFAULT_SWEEP_INTERVAL).await;
            next.await;
            assert_eq!(probe.calls.load(Ordering::SeqCst), expected);
            assert_eq!(probe.active.load(Ordering::SeqCst), 1);
        }
        shutdown.send(true).unwrap();
        probe.release.notify_waiters();
        scheduler.await.unwrap();
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_sweeps_are_bounded_and_shutdown_cancels_unfinished_children() {
        let probe = blocking_probe(true);
        let (shutdown, receiver) = watch::channel(false);
        let first = probe.called.notified();
        let mut scheduler =
            SandboxScheduler::with_interval(Arc::clone(&probe), DEFAULT_SWEEP_INTERVAL);
        scheduler.concurrent_sweeps = SANDBOX_CONCURRENT_SWEEPS;
        let scheduler = scheduler.spawn(receiver);
        first.await;
        for _ in 1..SANDBOX_CONCURRENT_SWEEPS {
            let next = probe.called.notified();
            tokio::time::advance(DEFAULT_SWEEP_INTERVAL).await;
            next.await;
        }
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            probe.calls.load(Ordering::SeqCst),
            SANDBOX_CONCURRENT_SWEEPS
        );
        assert_eq!(
            probe.active.load(Ordering::SeqCst),
            SANDBOX_CONCURRENT_SWEEPS
        );
        shutdown.send(true).unwrap();
        scheduler.await.unwrap();
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        assert_eq!(
            probe.calls.load(Ordering::SeqCst),
            SANDBOX_CONCURRENT_SWEEPS
        );
    }

    #[tokio::test(start_paused = true)]
    async fn maintenance_sweeps_remain_serial() {
        let probe = blocking_probe(true);
        let first = probe.called.notified();
        let (shutdown, receiver) = watch::channel(false);
        let scheduler = SandboxScheduler::with_interval(Arc::clone(&probe), DEFAULT_SWEEP_INTERVAL)
            .spawn(receiver);
        first.await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        shutdown.send(true).unwrap();
        probe.release.notify_waiters();
        scheduler.await.unwrap();
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    }
}
