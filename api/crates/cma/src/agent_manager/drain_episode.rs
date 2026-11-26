use std::{future::Future, sync::Mutex, time::Duration};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrainCause {
    PreStop,
    Shutdown,
    OwnershipFailure,
    HttpFailure,
}

impl DrainCause {
    fn label(self) -> &'static str {
        match self {
            Self::PreStop => "prestop",
            Self::Shutdown => "shutdown",
            Self::OwnershipFailure => "ownership_failure",
            Self::HttpFailure => "http_failure",
        }
    }
}

pub(super) struct DrainEpisode {
    pub until: Instant,
    started: Instant,
    cause: DrainCause,
    driver: tokio::sync::Mutex<()>,
    outcome: Mutex<Option<Result<(), String>>>,
}

impl DrainEpisode {
    pub fn new(cause: DrainCause, limit: Instant) -> Self {
        let started = Instant::now();
        Self {
            until: limit.min(started + Duration::from_secs(23)),
            started,
            cause,
            driver: tokio::sync::Mutex::new(()),
            outcome: Mutex::new(None),
        }
    }

    pub async fn run(&self, work: impl Future<Output = Result<(), String>>) -> Result<(), String> {
        if let Some(outcome) = self.outcome.lock().unwrap().clone() {
            return outcome;
        }
        let run = async {
            let _driver = self.driver.lock().await;
            if let Some(outcome) = self.outcome.lock().unwrap().clone() {
                return outcome;
            }
            self.retain(work.await)
        };
        let result = if Instant::now() < self.until {
            tokio::time::timeout_at(self.until, run)
                .await
                .unwrap_or_else(|_| Err(timeout()))
        } else {
            Err(timeout())
        };
        self.retain(result)
    }

    fn retain(&self, result: Result<(), String>) -> Result<(), String> {
        let mut outcome = self.outcome.lock().unwrap();
        if outcome.is_none() {
            tracing::info!(parent: None,
                stage = "shutdown",
                cause = self.cause.label(),
                outcome = if result.is_ok() {
                    "complete"
                } else {
                    "incomplete"
                },
                elapsed_ms = self.started.elapsed().as_millis(),
                remaining_ms = self
                    .until
                    .saturating_duration_since(Instant::now())
                    .as_millis(),
                "Agent Manager drain finished"
            );
            *outcome = Some(result);
        }
        outcome.as_ref().unwrap().clone()
    }
}

fn timeout() -> String {
    "Agent Manager drain exceeded its original deadline".into()
}

#[cfg(test)]
#[path = "drain_episode_tests.rs"]
mod tests;
