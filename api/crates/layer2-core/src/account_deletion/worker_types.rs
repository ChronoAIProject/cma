use super::{AccountDeletionId, ErasureDigest, ErasureStep};
use serde::{Deserialize, Serialize};

pub const JOURNAL_VERSION: u32 = 1;
pub const MAX_PLAN_TARGETS: usize = 500;
pub const MAX_JOURNAL_BYTES: usize = 1024 * 1024;
pub const MAX_LEASE_SECONDS: u64 = 300;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalContract {
    pub version: u32,
    pub worker: ErasureDigest,
    pub catalogue: ErasureDigest,
    pub policy: ErasureDigest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalBinding {
    pub deletion_id: AccountDeletionId,
    pub contract: JournalContract,
    pub sequence: u64,
    pub inventory_generation: u64,
    pub step: ErasureStep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetClass {
    Record,
    Runtime,
    Home,
    Object,
    AccountKey,
    Delegation,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetReference {
    pub class: TargetClass,
    pub namespace: String,
    pub identity: String,
    pub incarnation: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPage {
    pub targets: Vec<TargetReference>,
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState {
    Unresolved,
    Observed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalBlock {
    AuthorityUnavailable,
    CapacityExceeded,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalCommand {
    Enroll,
    Claim {
        request_id: String,
        holder: String,
        lease_seconds: u64,
    },
    Prepare {
        page: PlanPage,
    },
    Observe {
        plan: String,
        evidence: ErasureDigest,
        state: ObservationState,
    },
    Renew {
        lease_seconds: u64,
    },
    Defer {
        delay_seconds: u64,
    },
    Block {
        reason: JournalBlock,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalCheckpoint {
    pub binding: JournalBinding,
    pub parent_revision: u64,
    pub last_operation: String,
    pub latest_plan: Option<String>,
    pub last_observation: Option<String>,
    pub digest: ErasureDigest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalReceipt {
    pub operation: String,
    pub checkpoint: JournalCheckpoint,
    pub parent_fence: Option<u64>,
    pub lease_expires_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    #[error("erasure journal input is invalid")]
    Invalid,
    #[error("erasure journal capacity exceeded")]
    Capacity,
    #[error("erasure journal is not explicitly prepared")]
    Unprepared,
    #[error("erasure journal version or stored binding is incompatible")]
    Incompatible,
    #[error("erasure journal operation binding differs")]
    BindingMismatch,
    #[error("erasure journal changed")]
    Conflict,
    #[error("erasure journal claim is stale")]
    StaleClaim,
    #[error("erasure journal is unavailable")]
    Unavailable,
    #[error("erasure journal operation stopped before commit submission")]
    Cancelled,
}
