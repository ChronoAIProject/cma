//! Bounded local task custody; durable claims remain authoritative after cancellation.
use std::{future::Future, sync::Arc, sync::Mutex, time::Duration};
use tokio::{sync::watch, task::JoinSet};

#[derive(Clone)]
pub(crate) struct TaskScope(Arc<State>);

struct State {
    tasks: Mutex<Option<JoinSet<()>>>,
    completed: watch::Sender<bool>,
    capacity: usize,
    cancel_on_close: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("lifecycle tasks have not completed cancellation before the drain deadline")]
pub(crate) struct DrainTimeout;

impl TaskScope {
    #[cfg(test)]
    pub(crate) fn fixture(capacity: usize) -> Self {
        Self::new(capacity)
    }

    #[cfg(test)]
    pub(crate) fn spawn_fixture(&self, task: impl Future<Output = ()> + Send + 'static) -> bool {
        self.spawn(task)
    }

    pub(super) fn new(capacity: usize) -> Self {
        Self::with_close_policy(capacity, true)
    }

    pub(crate) fn completing(capacity: usize) -> Self {
        Self::with_close_policy(capacity, false)
    }

    fn with_close_policy(capacity: usize, cancel_on_close: bool) -> Self {
        Self(Arc::new(State {
            tasks: Mutex::new(Some(JoinSet::new())),
            completed: watch::channel(false).0,
            capacity,
            cancel_on_close,
        }))
    }

    pub(super) fn is_closed(&self) -> bool {
        self.0.tasks.lock().unwrap().is_none()
    }

    /// Admission and closure share one short lock, never held across IO.
    pub(crate) fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) -> bool {
        let mut state = self.0.tasks.lock().unwrap();
        let Some(tasks) = state.as_mut() else {
            return false;
        };
        while let Some(result) = tasks.try_join_next() {
            report(result);
        }
        if tasks.len() >= self.0.capacity {
            return false;
        }
        tasks.spawn(task);
        true
    }

    pub(crate) fn close(&self) {
        let tasks = self.0.tasks.lock().unwrap().take();
        if let Some(mut tasks) = tasks {
            tracing::info!(tasks = tasks.len(), "closing lifecycle task admission");
            if self.0.cancel_on_close {
                tasks.abort_all();
            }
            let signal = self.0.completed.clone();
            // This supervisor owns only joins. A cancelled drain caller or a
            // timeout must not lose custody and let a later drain report success
            // while an uncooperative task is still executing.
            tokio::spawn(async move {
                while let Some(result) = tasks.join_next().await {
                    report(result);
                }
                signal.send_replace(true);
                tracing::info!("lifecycle tasks finished local cancellation");
            });
        }
    }

    pub(crate) async fn drain(&self, limit: Duration) -> Result<(), DrainTimeout> {
        let mut completed = self.0.completed.subscribe();
        self.close();
        let finished = matches!(
            tokio::time::timeout(limit, completed.wait_for(|done| *done)).await,
            Ok(Ok(_))
        );
        if finished {
            Ok(())
        } else {
            tracing::warn!("lifecycle drain incomplete; durable claims remain retained");
            Err(DrainTimeout)
        }
    }
}

fn report(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        if !error.is_cancelled() {
            tracing::error!(%error, "tracked lifecycle task failed");
        }
    }
}

#[cfg(test)]
#[path = "agent_lifecycle_tasks_tests.rs"]
mod tests;
