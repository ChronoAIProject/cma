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
    lifecycles: Vec<crate::agent_lifecycle::LifecycleDrain>,
}

impl ShutdownCoordinator {
    pub(crate) fn new() -> Self {
        let (signal, _) = watch::channel(false);
        Self {
            signal,
            workers: Vec::new(),
            drain_timeout: WORKER_DRAIN_TIMEOUT,
            lifecycles: Vec::new(),
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

    pub(crate) fn register_lifecycle(&mut self, lifecycle: crate::agent_lifecycle::LifecycleDrain) {
        self.lifecycles.push(lifecycle);
    }

    pub(crate) async fn drain_lifecycles(
        &self,
        limit: Duration,
    ) -> Result<(), crate::agent_lifecycle::LifecycleDrainTimeout> {
        let deadline = Instant::now() + limit;
        for lifecycle in &self.lifecycles {
            lifecycle.close();
        }
        for lifecycle in &self.lifecycles {
            lifecycle
                .drain(deadline.saturating_duration_since(Instant::now()))
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn drain_agent_manager(
        &self,
        manager: Option<&crate::agent_manager::AgentManager>,
    ) -> Result<(), String> {
        let Some(manager) = manager else {
            return Ok(());
        };
        manager.stop_admission();
        let draining = async {
            self.drain_lifecycles(Duration::from_secs(5))
                .await
                .map_err(|error| error.to_string())?;
            manager.drain().await
        };
        match tokio::time::timeout(Duration::from_secs(23), draining).await {
            Ok(Ok(())) => Ok(()),
            result => {
                manager.coordinator.fail().await;
                Err(match result {
                    Ok(Err(reason)) => reason,
                    _ => "Agent Manager shutdown did not confirm drain before its deadline".into(),
                })
            }
        }
    }

    pub(crate) fn signal(&self) {
        for lifecycle in &self.lifecycles {
            lifecycle.close();
        }
        let _ = self.signal.send(true);
    }

    pub(crate) async fn drain(self) -> usize {
        self.signal();
        let deadline = Instant::now() + self.drain_timeout;
        let incomplete_lifecycles =
            usize::from(self.drain_lifecycles(self.drain_timeout).await.is_err());
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

        let aborted = overdue.len() + incomplete_lifecycles;
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

pub(crate) use cma_am_auth::shutdown::signalled;

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

mod signals;
pub(crate) use signals::shutdown_signal;

#[path = "shutdown/contained.rs"]
mod contained;
pub(crate) use contained::serve_contained;
