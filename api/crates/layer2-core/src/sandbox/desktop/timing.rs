//! Optional scalar phase timings keep authorization and provider costs distinct.
use std::future::Future;
use tokio::time::Instant;

pub(super) async fn observe<T, E>(
    phase: &'static str,
    operation: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut timer = Timer {
        phase,
        started: tracing::enabled!(target: "cma::desktop::connect", tracing::Level::DEBUG)
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
            tracing::debug!(
                target: "cma::desktop::connect",
                phase = self.phase,
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                outcome = self.outcome,
                "desktop connection phase completed"
            );
        }
    }
}

#[cfg(test)]
mod tests;
