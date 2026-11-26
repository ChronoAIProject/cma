use crate::SubjectId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

/// Personal session evidence established at the authentication boundary, never persisted as a capability.
#[derive(Debug, Clone)]
pub struct OwnerSessionAuthority {
    subject: SubjectId,
}
impl OwnerSessionAuthority {
    pub fn verified_personal(subject: SubjectId) -> Self {
        Self { subject }
    }
    pub fn subject(&self) -> &SubjectId {
        &self.subject
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountDeletionId(pub String);
impl AccountDeletionId {
    pub fn generate() -> Self {
        Self(format!("acd_{}", uuid::Uuid::new_v4().simple()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Display for AccountDeletionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErasureStep {
    Inventory,
    RetireSandboxes,
    ReleaseHomes,
    EraseRecords,
    EraseObjects,
    RevokeCredentials,
    VerifyNoResidue,
}
pub const ERASURE_ORDER: [ErasureStep; 7] = [
    ErasureStep::Inventory,
    ErasureStep::RetireSandboxes,
    ErasureStep::ReleaseHomes,
    ErasureStep::EraseRecords,
    ErasureStep::EraseObjects,
    ErasureStep::RevokeCredentials,
    ErasureStep::VerifyNoResidue,
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountDeletionStatus {
    Running,
    Blocked,
    Completed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErasureFailureCode {
    Unavailable,
    RateLimited,
    PermissionDenied,
    Unsupported,
    InvalidResponse,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErasureBlock {
    OwnerAuthorityUnavailable,
    /// Permanent bounded-input refusal; isolate the attributed cohort member.
    CapacityExceeded {
        class: String,
        limit: u64,
    },
    /// The attributed record requires repair of its ownership evidence.
    CorruptRecord {
        collection: String,
    },
    /// Historical C1 record/event format; keep the exact wire name and fields.
    /// The C2 publisher executor no longer emits this refusal.
    RetainedReferencePending {
        collection: String,
    },
    /// Keep the C1/C2 record/event format when replacing identity handling.
    /// C2 still emits this refusal; later identity executors retain only its reader.
    IdentityRedactionPending {
        collection: String,
    },
    RetirementArchiveHold {
        sandbox_id: String,
    },
    SandboxRetirementFailed {
        sandbox_id: String,
    },
    HomeNotFinal {
        sandbox_id: String,
    },
    ObjectStoreRefused {
        reason: ErasureFailureCode,
    },
    BillingSettlementPending,
    /// A name from the static erasure catalog, never caller-supplied text.
    UnsupportedResidue {
        collection: String,
    },
    /// Static residue class names supplied by the reconciler.
    ResidueFound {
        classes: Vec<String>,
    },
}
impl ErasureBlock {
    pub(super) fn for_audit(&self) -> Self {
        let hash = |id: &str| crate::document_artifacts::digest(id.as_bytes());
        match self {
            Self::RetirementArchiveHold { sandbox_id } => Self::RetirementArchiveHold {
                sandbox_id: hash(sandbox_id),
            },
            Self::SandboxRetirementFailed { sandbox_id } => Self::SandboxRetirementFailed {
                sandbox_id: hash(sandbox_id),
            },
            Self::HomeNotFinal { sandbox_id } => Self::HomeNotFinal {
                sandbox_id: hash(sandbox_id),
            },
            other => other.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountDeletionError {
    #[error("account deletion is unavailable")]
    Unavailable,
    #[error("irreversible deletion must be acknowledged")]
    AcknowledgementRequired,
    #[error("account deletion requires an owner session")]
    RequiresOwnerSession,
    #[error("account deletion was not found")]
    NotFound,
    #[error("account deletion changed")]
    Conflict,
    #[error("invalid account deletion transition")]
    Invalid,
    #[error("invalid idempotency key")]
    InvalidIdempotencyKey,
    #[error("account deletion lease is stale")]
    StaleFence,
    #[error("account deletion completion requires an independently verified proof")]
    VerificationRequired,
    #[error("replay the same request to reconcile its commit")]
    RecoveryRequired,
    #[error("account deletion is in progress")]
    InProgress,
    #[error("account deletion store unavailable: {0}")]
    Store(String),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureLease {
    pub holder: String,
    pub fence: u64,
    pub token: String,
    pub expires_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureWork {
    pub ready_at: String,
    pub attempts: u32,
    pub next_fence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<ErasureLease>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureInventory {
    pub sandboxes: Vec<String>,
    pub workspaces: Vec<String>,
    pub worktrees: Vec<String>,
    pub agents: Vec<String>,
    pub launches: Vec<String>,
    pub bindings: Vec<String>,
    pub triggers: Vec<String>,
    pub bots: Vec<String>,
    pub documents: Vec<String>,
    pub homes: Vec<String>,
    pub pending_key_ids: Vec<String>,
    pub counts: BTreeMap<String, u64>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyReceipt {
    pub remaining: u64,
    pub empty_at: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidueReport {
    pub records: BTreeMap<String, u64>,
    pub objects: u64,
    pub homes: Vec<String>,
    pub keys: KeyReceipt,
}
impl ResidueReport {
    pub fn is_zero(&self) -> bool {
        self.records.values().all(|count| *count == 0)
            && self.objects == 0
            && self.homes.is_empty()
            && self.keys.remaining == 0
            && self
                .keys
                .empty_at
                .as_deref()
                .and_then(crate::seconds_from_rfc3339)
                .is_some()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErasureEventKind {
    Started,
    Completed { counts: BTreeMap<String, u64> },
    Retried { failure: ErasureFailureCode },
    Blocked { reason: ErasureBlock },
    Rearmed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureEvent {
    pub deletion_id: AccountDeletionId,
    pub seq: u64,
    pub at: String,
    pub step: ErasureStep,
    pub kind: ErasureEventKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepProgress {
    pub step: ErasureStep,
    pub completed_at: String,
    pub counts: BTreeMap<String, u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub id: AccountDeletionId,
    pub status: AccountDeletionStatus,
    pub step: ErasureStep,
    pub requested_at: String,
    pub deadline_at: String,
    pub completed_at: Option<String>,
    pub steps: Vec<StepProgress>,
}
pub enum StepOutcome {
    Completed { counts: BTreeMap<String, u64> },
    Pending { retry_after: Duration },
}
pub enum ErasureFailure {
    Transient { reason: ErasureFailureCode },
    Blocked { reason: ErasureBlock },
}
pub fn subject_hash(subject: &SubjectId) -> String {
    crate::document_artifacts::digest(subject.as_str().as_bytes())
}
