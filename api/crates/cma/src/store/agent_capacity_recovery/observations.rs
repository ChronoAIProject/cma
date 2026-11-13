//! Bounded discovery diagnostics; no candidate or driver text is retained.
use std::time::Instant;

use layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError;
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};

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
    rows: u32,
    code: Option<i32>,
    transient: bool,
    unknown_commit: bool,
}

impl Observation {
    pub(in crate::store) fn new(phase: Phase) -> Self {
        Self {
            phase,
            // A polled operation dropped before a result must remain visible.
            outcome: "cancelled",
            started: Instant::now(),
            rows: 0,
            code: None,
            transient: false,
            unknown_commit: false,
        }
    }

    pub(in crate::store) fn phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    pub(in crate::store) fn row(&mut self) {
        self.rows = self.rows.saturating_add(1);
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
        self.outcome = "driver";
        self.code = match error.kind.as_ref() {
            ErrorKind::Command(error) => Some(error.code),
            ErrorKind::Write(WriteFailure::WriteError(error)) => Some(error.code),
            _ => None,
        };
        self.transient = error.contains_label(TRANSIENT_TRANSACTION_ERROR);
        self.unknown_commit = error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT);
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
        if matches!(outcome, "deadline" | "driver" | "invalid_evidence") {
            tracing::warn!(
                phase,
                reason = outcome,
                driver_code = self.code,
                transient_transaction = self.transient,
                unknown_commit = self.unknown_commit,
                rows_seen = self.rows,
                elapsed_ms = elapsed.as_millis() as u64,
                "Accepted capacity recovery discovery failed"
            );
        } else {
            tracing::debug!(
                phase,
                outcome,
                rows_seen = self.rows,
                elapsed_ms = elapsed.as_millis() as u64,
                "Accepted capacity recovery discovery observed"
            );
        }
    }
}

#[cfg(test)]
mod tests;
