//! Call-local, payload-free observations; timing never supplies runtime authority.
use std::{future::Future, time::Instant};

struct Phase<'a> {
    sandbox_id: &'a str,
    phase: &'static str,
    started: Instant,
    completed: bool,
    succeeded: bool,
}

impl Drop for Phase<'_> {
    fn drop(&mut self) {
        tracing::info!(
            sandbox_id = self.sandbox_id,
            phase = self.phase,
            elapsed_ms = self.started.elapsed().as_millis(),
            completed = self.completed,
            succeeded = self.succeeded,
            "Native runtime inspection phase ended"
        );
    }
}

pub(super) async fn measure<T>(
    sandbox_id: &str,
    phase: &'static str,
    operation: impl Future<Output = crate::Result<T>>,
) -> crate::Result<T> {
    let mut timing = Phase {
        sandbox_id,
        phase,
        started: Instant::now(),
        completed: false,
        succeeded: false,
    };
    let result = operation.await;
    timing.completed = true;
    timing.succeeded = result.is_ok();
    result
}
