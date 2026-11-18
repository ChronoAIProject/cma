//! Fresh per-attempt correlation, independent of event identity and credentials.
use super::ContextFailure;
use crate::providers::EventContextSnapshot;
use tokio::time::Instant;

pub(in crate::providers) const HEADER: &str = "x-cma-context-attempt";

pub(in crate::providers) fn lookup() -> String {
    let hash = crate::telemetry::http::correlation_hash(uuid::Uuid::new_v4().as_bytes());
    // The ingress span already contains the hashed request correlation. This
    // single link joins its attempts; phase records below never inherit spans.
    tracing::info!(lookup_hash = hash, "provider context lookup started");
    hash
}

pub(in crate::providers) struct Attempt {
    pub header: String,
    hash: String,
    lookup: String,
    number: u8,
    started: Instant,
    phase: &'static str,
    outcome: &'static str,
    reason: &'static str,
    pub status: Option<u16>,
}

impl Attempt {
    pub fn new(lookup: &str, number: u8) -> Self {
        let header = uuid::Uuid::new_v4().simple().to_string();
        let value = Self {
            hash: crate::telemetry::http::correlation_hash(header.as_bytes()),
            header,
            lookup: lookup.to_owned(),
            number,
            started: Instant::now(),
            phase: "credential_tracking",
            outcome: "cancelled",
            reason: "future_dropped",
            status: None,
        };
        value.log("started");
        value
    }

    pub fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
    }

    pub fn finish(&mut self, result: &Result<Option<EventContextSnapshot>, ContextFailure>) {
        (self.outcome, self.reason) = match result {
            Ok(Some(_)) => ("completed", "retained"),
            Ok(None) => ("completed", "absent"),
            Err(failure) => ("error", failure.class),
        };
    }

    fn log(&self, state: &'static str) {
        tracing::info!(
            parent: None,
            lookup_hash = self.lookup,
            attempt_hash = self.hash,
            attempt = self.number,
            phase = self.phase,
            outcome = state,
            reason = if state == "started" { "none" } else { self.reason },
            upstream_status = self.status,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "provider context attempt"
        );
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.log(self.outcome);
    }
}

#[cfg(test)]
#[path = "context_diagnostics_tests.rs"]
mod tests;
