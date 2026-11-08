//! Cooperative shutdown and bounded background-worker drain.

use std::time::Duration;

use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{timeout_at, Instant},
};

const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct ShutdownCoordinator {
    signal: watch::Sender<bool>,
    workers: Vec<JoinHandle<()>>,
    drain_timeout: Duration,
}

impl ShutdownCoordinator {
    pub(crate) fn new() -> Self {
        let (signal, _) = watch::channel(false);
        Self {
            signal,
            workers: Vec::new(),
            drain_timeout: WORKER_DRAIN_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_drain_timeout(drain_timeout: Duration) -> Self {
        let mut coordinator = Self::new();
        coordinator.drain_timeout = drain_timeout;
        coordinator
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.signal.subscribe()
    }

    pub(crate) fn push(&mut self, worker: JoinHandle<()>) {
        self.workers.push(worker);
    }

    pub(crate) fn signal(&self) {
        let _ = self.signal.send(true);
    }

    pub(crate) async fn drain(self) -> usize {
        self.signal();
        let deadline = Instant::now() + self.drain_timeout;
        let mut workers = self.workers.into_iter();
        let mut overdue = Vec::new();
        while let Some(mut worker) = workers.next() {
            match timeout_at(deadline, &mut worker).await {
                Ok(result) => report(result),
                Err(_) => {
                    overdue.push(worker);
                    overdue.extend(workers);
                    break;
                }
            }
        }

        let aborted = overdue.len();
        for worker in &overdue {
            worker.abort();
        }
        for worker in overdue {
            report(worker.await);
        }
        // Joined workers have released their read guards, including cancellation
        // at the drain deadline. Export those final totals before returning.
        crate::read_telemetry::emit();
        aborted
    }
}

pub(crate) async fn signalled(mut signal: watch::Receiver<bool>) {
    while !*signal.borrow() {
        if signal.changed().await.is_err() {
            break;
        }
    }
}

fn report(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        if error.is_cancelled() {
            tracing::warn!("background worker exceeded its shutdown drain deadline");
        } else {
            tracing::error!(error = %error, "background worker terminated unexpectedly");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cooperative_worker_is_signalled_and_joined() {
        let mut coordinator = ShutdownCoordinator::new();
        let signal = coordinator.subscribe();
        coordinator.push(tokio::spawn(signalled(signal)));

        assert_eq!(coordinator.drain().await, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn uncooperative_worker_is_aborted_after_the_total_deadline() {
        let mut coordinator = ShutdownCoordinator::with_drain_timeout(Duration::from_secs(1));
        coordinator.push(tokio::spawn(std::future::pending()));
        let draining = tokio::spawn(coordinator.drain());
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(draining.await.unwrap(), 1);
    }
}
