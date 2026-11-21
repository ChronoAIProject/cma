//! Bounded discovery metrics independent of caller span formatting.
use std::time::Instant;

use layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError;
use mongodb::error::{Error, ErrorKind};

const MAX_TIME_MS_EXPIRED: i32 = 50;

#[derive(Clone, Copy)]
pub(in crate::store) enum Phase {
    Presence,
    AdmittedQuery,
    ExpiredQuery,
    CandidateValidation,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Presence => "presence",
            Self::AdmittedQuery => "admitted_query",
            Self::ExpiredQuery => "expired_query",
            Self::CandidateValidation => "candidate_validation",
        }
    }
}

pub(in crate::store) struct Observation {
    phase: Phase,
    outcome: &'static str,
    started: Instant,
}

impl Observation {
    pub(in crate::store) fn new(phase: Phase) -> Self {
        Self {
            phase,
            // A polled operation dropped before a result must remain visible.
            outcome: "cancelled",
            started: Instant::now(),
        }
    }

    pub(in crate::store) fn phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    pub(in crate::store) fn complete(&mut self) {
        self.outcome = "success";
    }

    pub(in crate::store) fn absent(&mut self) {
        self.outcome = "absent";
    }

    pub(in crate::store) fn deadline(&mut self) -> RecoveryError {
        self.outcome = "deadline";
        RecoveryError::Unavailable
    }

    pub(in crate::store) fn invalid(&mut self) {
        self.phase = Phase::CandidateValidation;
        self.outcome = "invalid_evidence";
    }

    pub(in crate::store) fn driver(&mut self, error: &Error) -> RecoveryError {
        self.outcome = match error.kind.as_ref() {
            ErrorKind::Command(error) if error.code == MAX_TIME_MS_EXPIRED => "deadline",
            _ => "driver",
        };
        RecoveryError::Unavailable
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        let phase = self.phase.label();
        let outcome = self.outcome;
        let elapsed = self.started.elapsed();
        metrics::counter!("cma_capacity_recovery_discovery_total", "phase" => phase,
            "outcome" => outcome)
        .increment(1);
        metrics::histogram!("cma_capacity_recovery_discovery_seconds", "phase" => phase)
            .record(elapsed.as_secs_f64());
    }
}

#[cfg(test)]
mod tests;
