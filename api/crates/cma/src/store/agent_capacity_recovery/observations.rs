//! Bounded discovery metrics independent of caller span formatting.
use std::time::Instant;

use layer2_core::sandbox::agent_capacity_recovery::{
    ports::{CommitOutcome, RecoveryError},
    CapacityRecoveryDisposition,
};
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

#[derive(Clone, Copy)]
pub(in crate::store) enum Stage {
    Scanned,
    Eligible,
    Admitted,
    Completed,
    Skipped,
    Blocked,
}

#[derive(Clone, Copy)]
pub(in crate::store) enum Outcome {
    Observed,
    Committed,
    Replay,
    Refused,
    KnownAbort,
    Unknown,
}

pub(in crate::store) fn cohort(stage: Stage, outcome: Outcome) {
    let stage = match stage {
        Stage::Scanned => "scanned",
        Stage::Eligible => "eligible",
        Stage::Admitted => "admitted",
        Stage::Completed => "completed",
        Stage::Skipped => "skipped",
        Stage::Blocked => "blocked",
    };
    let outcome = match outcome {
        Outcome::Observed => "observed",
        Outcome::Committed => "committed",
        Outcome::Replay => "replay",
        Outcome::Refused => "refused",
        Outcome::KnownAbort => "known_abort",
        Outcome::Unknown => "unknown",
    };
    metrics::counter!("cma_capacity_recovery_cohort_total", "stage" => stage,
        "outcome" => outcome)
    .increment(1);
}

pub(super) fn preflight(disposition: CapacityRecoveryDisposition) {
    use CapacityRecoveryDisposition as D;
    cohort(Stage::Scanned, Outcome::Observed);
    let (stage, outcome) = match disposition {
        D::EvidenceRequired | D::ContinueOriginalWork => (Stage::Eligible, Outcome::Observed),
        D::CleanupOnly | D::TerminalPreserved | D::NotCapacityFailure => {
            (Stage::Skipped, Outcome::Observed)
        }
        _ => (Stage::Blocked, Outcome::Refused),
    };
    cohort(stage, outcome);
}

pub(super) fn commit<T>(stage: Stage, result: &Result<CommitOutcome<T>, RecoveryError>) {
    match result {
        Ok(CommitOutcome::Recorded(_)) => cohort(stage, Outcome::Committed),
        Ok(CommitOutcome::KnownAbort) => cohort(Stage::Blocked, Outcome::KnownAbort),
        Ok(CommitOutcome::Unknown) => cohort(Stage::Blocked, Outcome::Unknown),
        Err(RecoveryError::Unauthorized) => {}
        Err(_) => cohort(Stage::Blocked, Outcome::Refused),
    }
}

#[cfg(test)]
mod tests;
