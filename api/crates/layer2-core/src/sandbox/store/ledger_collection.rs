//! Bounded collector progress. A lease is scheduling ownership, never an effect permit.
use serde::{Deserialize, Serialize};

const FORMAT: &str = "runtime-ledger-collector-progress-v1";
const MAX_CURSOR: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerCollectionPhase {
    Selected,
    Prepared,
    SourceCommitted,
    Acknowledged,
    Retained,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCollectionPending {
    pub operation_id: String,
    pub group_key: String,
    pub request_sha256: String,
    pub observation_sha256: String,
    pub source_sha256: String,
    pub phase: LedgerCollectionPhase,
    /// Exact original A receipt bytes are retained separately; this pins them.
    pub receipt_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCollectionLease {
    pub holder_sha256: String,
    pub fence: u64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCollectionPageProgress {
    pub page_sha256: String,
    pub active_revision: u64,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub remaining_groups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCollectorProgressV1 {
    pub format: String,
    pub revision: u64,
    pub binding_sha256: String,
    pub maintenance_epoch: u64,
    pub active_revision: u64,
    pub sweep: u64,
    pub cursor: Option<String>,
    pub complete: bool,
    pub lease: LedgerCollectionLease,
    pub page: Option<LedgerCollectionPageProgress>,
    pub pending: Option<LedgerCollectionPending>,
    /// At most one cold archive reference; no per-group hot tombstone list.
    pub last_ack_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerCollectionProgressError {
    Invalid,
    StaleClaim,
    PendingTransition,
    StaleCursor,
}

type Result<T> = std::result::Result<T, LedgerCollectionProgressError>;

#[derive(Debug, Clone)]
pub enum LedgerCollectorMutation {
    Claim {
        holder_sha256: String,
        now: i64,
        expires_at: i64,
    },
    PinPage {
        fence: u64,
        now: i64,
        page: LedgerCollectionPageProgress,
    },
    Select {
        fence: u64,
        now: i64,
        pending: LedgerCollectionPending,
    },
    Outcome {
        fence: u64,
        now: i64,
        operation_id: String,
        phase: LedgerCollectionPhase,
        receipt_sha256: Option<String>,
    },
    Settle {
        fence: u64,
        now: i64,
        operation_id: String,
    },
    Advance {
        fence: u64,
        now: i64,
        active_revision: u64,
        page_sha256: String,
    },
    Rollover {
        fence: u64,
        now: i64,
        active_revision: u64,
    },
}

#[async_trait::async_trait]
pub trait LedgerCollectionStore: Send + Sync {
    async fn ledger_collection_progress(
        &self,
        sandbox: &crate::SandboxId,
    ) -> std::result::Result<Option<LedgerCollectorProgressV1>, super::SandboxStoreError>;

    /// Initialize a retention hold with a CAS on the parent. This grants no
    /// maintenance or runtime authority and does not release an existing hold.
    async fn retain_ledger_collection(
        &self,
        sandbox: &crate::SandboxId,
        expected_sandbox_revision: u64,
        initial: LedgerCollectorProgressV1,
    ) -> std::result::Result<(), super::SandboxStoreError>;

    async fn update_ledger_collection(
        &self,
        sandbox: &crate::SandboxId,
        expected_sandbox_revision: u64,
        expected_progress_revision: u64,
        mutation: LedgerCollectorMutation,
    ) -> std::result::Result<LedgerCollectorProgressV1, super::SandboxStoreError>;
}

#[path = "ledger_collection_progress.rs"]
mod progress;

#[cfg(test)]
#[path = "ledger_collection_tests.rs"]
mod tests;
