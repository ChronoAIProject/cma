//! Fixed, content-free markers for the independent executable diagnostic.
use std::time::Instant;

pub struct Trace {
    replicas: usize,
    ownership: bool,
    started: Instant,
}

impl Trace {
    pub fn new(replicas: usize, ownership: bool) -> Self {
        Self {
            replicas,
            ownership,
            started: Instant::now(),
        }
    }

    /// Ordinal identifies a fixture process, never a PID or user identifier.
    /// Elapsed values share only this cohort's monotonic clock origin.
    pub fn stage(&self, phase: &'static str, state: &'static str, ordinal: usize) {
        eprintln!(
            "replica_stage replicas={} ownership={} phase={phase} state={state} ordinal={ordinal} elapsed_ms={}",
            self.replicas, self.ownership, self.started.elapsed().as_millis()
        );
    }

    pub fn headers(&self, ordinal: usize, status: u16) {
        eprintln!(
            "replica_http replicas={} ownership={} ordinal={ordinal} status={status} elapsed_ms={}",
            self.replicas,
            self.ownership,
            self.started.elapsed().as_millis()
        );
    }
}
