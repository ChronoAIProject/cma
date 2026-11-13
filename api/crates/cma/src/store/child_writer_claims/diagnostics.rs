//! Attribute contention without retaining documents, identifiers or driver text.
use std::time::Instant;

use mongodb::error::{Error, ErrorKind, WriteFailure};

use super::ChildWriterClaim;

pub(super) struct ClaimDiagnostics {
    kind: &'static str,
    pub(super) phase: &'static str,
    attempt: usize,
    started: Instant,
}

impl ClaimDiagnostics {
    pub(super) fn new(claim: &ChildWriterClaim) -> Self {
        Self {
            kind: match claim {
                ChildWriterClaim::Agent(_) => "agent",
                ChildWriterClaim::Workspace(_) => "workspace",
            },
            phase: "start",
            attempt: 0,
            started: Instant::now(),
        }
    }

    pub(super) fn attempt(&mut self, attempt: usize) {
        self.attempt = attempt;
        self.phase = "start";
    }

    pub(super) fn transient(&self, error: &Error) -> String {
        let code = match error.kind.as_ref() {
            ErrorKind::Command(error) => Some(error.code),
            ErrorKind::Write(WriteFailure::WriteError(error)) => Some(error.code),
            _ => None,
        };
        tracing::info!(
            child_kind = self.kind,
            phase = self.phase,
            attempt = self.attempt,
            driver_code = code,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "Writer claim transaction definitively aborted"
        );
        // The scheduler logs exhaustion. Give it bounded diagnostic values,
        // never the driver's message, which may contain document content.
        format!(
            "kind={} phase={} attempt={} driver_code={code:?}",
            self.kind, self.phase, self.attempt
        )
    }
}
