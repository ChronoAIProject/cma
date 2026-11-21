//! Payload-free timings for the independent stages of opening a conversation.
use std::future::Future;
use std::time::Instant;

pub(super) struct StartupTiming {
    last: Instant,
    attempt_id: String,
    correlation_digest: Option<String>,
}

impl StartupTiming {
    pub(super) fn new() -> Self {
        Self::with_correlation(None)
    }

    pub(super) fn for_agent(correlation_digest: String) -> Self {
        Self::with_correlation(Some(correlation_digest))
    }

    fn with_correlation(correlation_digest: Option<String>) -> Self {
        Self {
            last: Instant::now(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            correlation_digest,
        }
    }

    pub(super) fn completed(&mut self, phase: &'static str) {
        let now = Instant::now();
        let elapsed_ms = now.duration_since(self.last).as_millis();
        if let Some(correlation_digest) = &self.correlation_digest {
            tracing::info!(
                parent: None,
                phase,
                elapsed_ms,
                attempt_id = %self.attempt_id,
                correlation_digest,
                "Conversation startup phase completed"
            );
        } else {
            tracing::info!(phase, elapsed_ms, "Conversation startup phase completed");
        }
        self.last = now;
    }

    pub(super) async fn subphase<T, E, F>(
        &self,
        branch: &'static str,
        operation: &'static str,
        future: F,
    ) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        let guard = SubphaseGuard::new(
            &self.attempt_id,
            self.correlation_digest.clone(),
            branch,
            operation,
        );
        let result = future.await;
        guard.finish(if result.is_ok() { "ok" } else { "error" });
        // Nested observations must not reset the enclosing phase's clock.
        result
    }
}

struct SubphaseGuard {
    started: Instant,
    branch: &'static str,
    operation: &'static str,
    attempt_id: String,
    correlation_digest: Option<String>,
    finished: bool,
}

impl SubphaseGuard {
    fn new(
        attempt_id: &str,
        correlation_digest: Option<String>,
        branch: &'static str,
        operation: &'static str,
    ) -> Self {
        Self {
            started: Instant::now(),
            branch,
            operation,
            attempt_id: attempt_id.to_string(),
            correlation_digest,
            finished: false,
        }
    }

    fn finish(mut self, outcome: &'static str) {
        self.finished = true;
        self.emit(outcome);
    }

    fn emit(&self, outcome: &'static str) {
        tracing::info!(
            parent: None,
            phase = "protocol_session_subphase",
            branch = self.branch,
            operation = self.operation,
            elapsed_ms = self.started.elapsed().as_millis(),
            outcome,
            attempt_id = %self.attempt_id,
            correlation_digest = self.correlation_digest.as_deref().unwrap_or("unknown"),
            "Conversation startup subphase completed"
        );
    }
}

#[cfg(test)]
#[path = "startup_timing_tests.rs"]
pub(super) mod tests;

impl Drop for SubphaseGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.emit("cancelled");
        }
    }
}
