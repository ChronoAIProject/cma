//! Fixed recovery observations. No identity, raw error, or caller span is recorded.
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Discovery,
    Opportunity,
    Snapshot,
    NativeBefore,
    NativeAfter,
    History,
    PublicationLog,
    Proof,
    Admission,
    Commit,
    ReconcileCommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Success,
    Cancelled,
    Deadline,
    Unavailable,
    NativeChanged,
    HomeChanged,
    BootMismatch,
    RestorePending,
    MissingHistory,
    CorruptHistory,
    LedgerHeld,
    LedgerFormatUnproved,
    PublishedGeneration,
    StaleChildren,
    Ineligible,
    BindingHeld,
    IncompleteAuthority,
    UnknownCommit,
    UnresolvedCommit,
}

impl Reason {
    pub fn adapter(error: &crate::AdapterError, refusal: Self) -> Self {
        match error {
            crate::AdapterError::Unavailable(_) | crate::AdapterError::AtCapacity(_) => {
                Self::Unavailable
            }
            _ => refusal,
        }
    }
}

/// A completed attempt is a returned operation, not a completed lifecycle.
/// Admission is counted only at the admission stage, after its receipt returns.
pub struct Observation {
    stage: Stage,
    started: Instant,
    reason: Reason,
    attempted: bool,
    completed: bool,
}

impl Observation {
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            started: Instant::now(),
            reason: Reason::Cancelled,
            attempted: true,
            completed: false,
        }
    }

    pub fn pending(&mut self, reason: Reason) {
        self.reason = reason;
    }

    pub fn skipped(&mut self, reason: Reason) {
        self.reason = reason;
        self.attempted = false;
    }

    pub fn finish(&mut self, reason: Reason) {
        self.reason = reason;
        self.completed = true;
    }

    /// Preserve the exact provider error while recording its bounded category.
    pub fn adapter<T>(&mut self, result: crate::Result<T>, refusal: Reason) -> crate::Result<T> {
        self.finish(match &result {
            Ok(_) => Reason::Success,
            Err(error) => Reason::adapter(error, refusal),
        });
        result
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        tracing::info!(parent: None,
            stage = ?self.stage, reason = ?self.reason,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            attempted = u64::from(self.attempted), completed = u64::from(self.completed),
            admitted = u64::from(self.stage == Stage::Admission && self.reason == Reason::Success),
            "unpublished recovery stage"
        );
    }
}
