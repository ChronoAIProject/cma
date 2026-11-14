//! Fixed persistence observations; request/process identity comes from the caller's span.
use super::RecordError;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Validate,
    Session,
    TransactionStart,
    ChannelGuard,
    PartitionGuard,
    RecordUpdate,
    RecordInsert,
    Commit,
    Backoff,
    IndexUpdate,
    IndexLegacyRead,
}

impl Phase {
    // Classify the client call, not whether the server received or committed it.
    fn io_class(self) -> &'static str {
        match self {
            Self::Validate | Self::Session | Self::TransactionStart | Self::Backoff => "local",
            Self::ChannelGuard | Self::PartitionGuard | Self::RecordUpdate | Self::RecordInsert => {
                "transaction_write"
            }
            Self::Commit => "commit",
            Self::IndexUpdate => "write",
            Self::IndexLegacyRead => "read",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Session => "session",
            Self::TransactionStart => "transaction_start",
            Self::ChannelGuard => "channel_guard",
            Self::PartitionGuard => "partition_guard",
            Self::RecordUpdate => "record_update",
            Self::RecordInsert => "record_insert",
            Self::Commit => "commit",
            Self::Backoff => "backoff",
            Self::IndexUpdate => "index_update",
            Self::IndexLegacyRead => "index_legacy_read",
        }
    }
}

pub(super) struct Observation {
    operation: &'static str,
    sequence: u64,
    parent: tracing::Span,
    started: tokio::time::Instant,
    position: Mutex<(u8, Phase)>,
    finished: AtomicBool,
}

impl Observation {
    pub(super) fn transaction() -> Self {
        Self::new("replica_write")
    }

    pub(super) fn index() -> Self {
        Self::new("admission_index")
    }

    fn new(operation: &'static str) -> Self {
        Self {
            operation,
            // This ordinal is process-local. Only the caller's process-instance
            // and request-attempt span makes it a cross-process correlation key.
            sequence: NEXT_OPERATION.fetch_add(1, Ordering::Relaxed),
            parent: tracing::Span::current(),
            started: tokio::time::Instant::now(),
            position: Mutex::new((0, Phase::Validate)),
            finished: AtomicBool::new(false),
        }
    }

    pub(super) fn attempt(&self, number: u8) {
        *self.position.lock().expect("observation position") = (number, Phase::Validate);
    }

    pub(super) fn phase(&self, phase: Phase) {
        self.position.lock().expect("observation position").1 = phase;
        self.emit(false, "start", "none", "unconfirmed", false, false);
    }

    pub(super) fn mongo_error(&self, error: &mongodb::error::Error) {
        let transient = error.contains_label("TransientTransactionError");
        let unknown = error.contains_label("UnknownTransactionCommitResult");
        let (class, commit) = if unknown {
            ("unknown_commit", "unknown")
        } else if transient {
            // The index update is not a transaction. A label there cannot prove
            // that a prior write/transaction was aborted.
            let commit = if self.operation == "replica_write" {
                "aborted"
            } else {
                "unknown"
            };
            ("transient_transaction", commit)
        } else {
            ("unlabelled_mongo", "unknown")
        };
        // Never format the Mongo error, server message, code name, record or URI.
        self.emit(true, "error", class, commit, transient, unknown);
    }

    pub(super) fn finish<T>(&self, result: &Result<T, RecordError>) {
        self.finished.store(true, Ordering::Relaxed);
        match result {
            Ok(_) => self.emit(false, "end", "none", "acknowledged", false, false),
            Err(error) => self.emit(true, "end", record_class(error), "unknown", false, false),
        }
    }

    pub(super) fn deadline(&self) {
        self.emit(true, "deadline", "deadline", "unknown", false, false);
    }

    fn emit(
        &self,
        warning: bool,
        boundary: &'static str,
        class: &'static str,
        commit: &'static str,
        transient: bool,
        unknown: bool,
    ) {
        let (attempt, phase) = *self.position.lock().expect("observation position");
        let elapsed_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let labels_observed = boundary == "error";
        let label_class = if labels_observed {
            class
        } else {
            "not_observed"
        };
        let outcome = match boundary {
            "start" => "pending",
            "end" if class == "none" => "ok",
            "deadline" => "deadline",
            "cancelled" => "cancelled",
            _ => "error",
        };
        macro_rules! event {
            ($level:expr) => {
                tracing::event!(parent: &self.parent, $level,
                    event = "eg_admission_record",
                    record_operation = self.operation,
                    record_operation_sequence = self.sequence,
                    transaction_attempt = attempt,
                    phase = phase.name(),
                    io_class = phase.io_class(),
                    boundary,
                    outcome,
                    error_class = class,
                    label_class,
                    commit_class = commit,
                    elapsed_ms,
                    mongo_labels_observed = labels_observed,
                    transient_transaction_label = transient,
                    unknown_commit_label = unknown,
                    "replica persistence observation"
                );
            };
        }
        if warning {
            event!(tracing::Level::WARN);
        } else {
            event!(tracing::Level::DEBUG);
        }
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Relaxed) {
            // An outer store/HTTP deadline may cancel us before our own timer.
            // Dropping the client future never proves server-side rollback.
            self.emit(true, "cancelled", "cancelled", "unknown", false, false);
        }
    }
}

fn record_class(error: &RecordError) -> &'static str {
    match error {
        RecordError::NotFound => "not_found",
        RecordError::Conflict { .. } => "conflict",
        RecordError::GuardConflict { .. } => "guard_conflict",
        RecordError::Rejected(_) => "rejected",
        RecordError::Unavailable(_) => "unavailable",
        RecordError::Malformed(_) => "malformed",
    }
}
