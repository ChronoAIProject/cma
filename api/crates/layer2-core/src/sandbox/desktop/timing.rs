//! Scalar timings keep connection setup and independent renewal costs distinct.
use std::{
    future::Future,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

pub(super) async fn observe<T, E>(
    phase: &'static str,
    operation: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut timer = Timer {
        phase,
        started: tracing::enabled!(target: "cma::desktop::connect", tracing::Level::INFO)
            .then(Instant::now),
        outcome: "cancelled",
    };
    let result = operation.await;
    timer.outcome = if result.is_ok() { "success" } else { "failure" };
    result
}

struct Timer {
    phase: &'static str,
    started: Option<Instant>,
    outcome: &'static str,
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            // Fixed internal phase names and scalar durations exclude credential,
            // runtime, user and resource identities from diagnostic output.
            tracing::info!(
                target: "cma::desktop::connect",
                parent: None,
                phase = self.phase,
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                outcome = self.outcome,
                "desktop connection phase completed"
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum RenewalOutcome {
    Renewed,
    Refused,
    Cancelled,
}

impl RenewalOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Renewed => "renewed",
            Self::Refused => "refused",
            Self::Cancelled => "cancelled",
        }
    }
}

pub(super) struct Renewal {
    started: Instant,
    started_at: Option<SystemTime>,
    outcome: RenewalOutcome,
}

impl Renewal {
    pub(super) fn start() -> Self {
        Self {
            started: Instant::now(),
            started_at: tracing::enabled!(target: "cma::desktop::renewal", tracing::Level::DEBUG)
                .then(SystemTime::now),
            outcome: RenewalOutcome::Cancelled,
        }
    }

    pub(super) fn finish(mut self, outcome: RenewalOutcome) {
        self.outcome = outcome;
    }
}

impl Drop for Renewal {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        let outcome = self.outcome.label();
        metrics::histogram!("cma_desktop_authority_renewal_seconds", "outcome" => outcome)
            .record(elapsed.as_secs_f64());
        // The release measurement can align these optional intervals to the
        // experiment clock without including an owner or connection identity.
        if let Some(started_at) = self.started_at {
            let started_unix_ms = started_at
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|value| u64::try_from(value.as_millis()).ok());
            tracing::debug!(
                target: "cma::desktop::renewal",
                parent: None,
                started_unix_ms,
                elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                outcome,
                "desktop authority renewal ended"
            );
        }
    }
}

#[cfg(test)]
mod tests;
