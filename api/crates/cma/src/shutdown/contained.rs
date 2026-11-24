use super::*;
use crate::agent_manager::{AgentManager, DrainCause};
use std::{future::Future, pin::Pin};

const PROCESS_LIMIT: Duration = Duration::from_secs(63);
const HTTP_LIMIT: Duration = Duration::from_secs(10);

pub(crate) async fn serve_contained(
    mut server: Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>,
    http_stop: watch::Sender<bool>,
    trigger: impl Future<Output = DrainCause>,
    manager: Option<&AgentManager>,
    shutdown: ShutdownCoordinator,
) -> Result<(), String> {
    let (cause, completed_http) = tokio::select! {
        result = &mut server => (DrainCause::HttpFailure, Some(http_result(result))),
        cause = trigger => (cause, None),
    };
    let until = Instant::now() + PROCESS_LIMIT;
    // Signal HTTP immediately, while its worker dependencies stay available
    // through receiver cleanup. Both cooperative stages are driven together.
    http_stop.send_replace(true);
    let manager_drain = async {
        match manager {
            Some(manager) => manager.drain_contained(cause, until).await,
            None => shutdown
                .drain_lifecycles(Duration::from_secs(5))
                .await
                .map_err(|_| "Local lifecycle drain was not confirmed".into()),
        }
    };
    let http_drain = async {
        match completed_http {
            Some(result) => result,
            None => match timeout_at(until.min(Instant::now() + HTTP_LIMIT), &mut server).await {
                Ok(result) => http_result(result),
                Err(_) => Err("HTTP server exceeded its shutdown drain deadline".into()),
            },
        }
    };
    let (manager_result, http_result) = tokio::join!(manager_drain, http_drain);
    drop(server);
    let workers = shutdown.drain_contained_workers(until).await;
    let cause_result = match cause {
        DrainCause::OwnershipFailure => Err("Agent Manager ownership maintenance failed".into()),
        _ => Ok(()),
    };
    cause_result
        .and(manager_result)
        .and(http_result)
        .and(workers)
}

fn http_result(result: std::io::Result<()>) -> Result<(), String> {
    result.map_err(|error| format!("HTTP server failed: {error}"))
}

impl ShutdownCoordinator {
    async fn drain_contained_workers(mut self, until: Instant) -> Result<(), String> {
        self.signal();
        let worker_until = until.min(Instant::now() + self.drain_timeout);
        let mut incomplete = self
            .drain_lifecycles(worker_until.saturating_duration_since(Instant::now()))
            .await
            .is_err();
        let mut overdue = Vec::new();
        for (index, worker) in self.workers.iter_mut().enumerate() {
            match timeout_at(worker_until, worker).await {
                Ok(result) => {
                    incomplete |= result.is_err();
                    report(result);
                }
                Err(_) => {
                    overdue.push(index);
                    incomplete = true;
                }
            }
        }
        for index in &overdue {
            self.workers[*index].abort();
        }
        let mut unconfirmed = 0;
        for index in overdue {
            match timeout_at(until, &mut self.workers[index]).await {
                Ok(result) => report(result),
                Err(_) => unconfirmed += 1,
            }
        }
        tracing::info!(parent: None,
            stage = "worker_shutdown",
            outcome = if incomplete { "incomplete" } else { "complete" },
            unconfirmed_tasks = unconfirmed,
            "Background worker shutdown finished"
        );
        if unconfirmed == 0 {
            crate::read_telemetry::emit();
        }
        if incomplete {
            Err("Background worker shutdown was not confirmed cleanly".into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "contained_tests.rs"]
mod tests;
