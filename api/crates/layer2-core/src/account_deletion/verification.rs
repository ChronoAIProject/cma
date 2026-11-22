//! Structural coverage contract, separate from independent proof authority and completion.
use super::{AccountDeletionId, ErasureDigest, ErasureEffectCheckpoint};
use serde::{Deserialize, Serialize};

mod validation;
pub use validation::{CoverageError, ValidatedCoverage};

pub const ERASURE_COVERAGE_VERSION: u32 = 1;
pub const MAX_ERASURE_COVERAGE_SCOPES: usize = 4096;
pub const MAX_ERASURE_SCAN_PAGES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidueClass {
    Records,
    Runtimes,
    Homes,
    Objects,
    AccountKeys,
    Delegations,
    Logs,
    Backups,
    DeletionEvidence,
}

pub const REQUIRED_RESIDUE_CLASSES: [ResidueClass; 9] = [
    ResidueClass::Records,
    ResidueClass::Runtimes,
    ResidueClass::Homes,
    ResidueClass::Objects,
    ResidueClass::AccountKeys,
    ResidueClass::Delegations,
    ResidueClass::Logs,
    ResidueClass::Backups,
    ResidueClass::DeletionEvidence,
];

/// Each source namespace and provider scope needs its own catalogue entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidueScope {
    pub class: ResidueClass,
    pub namespace: String,
    pub authority_scope: ErasureDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationBinding {
    pub deletion_id: AccountDeletionId,
    pub subject_digest: ErasureDigest,
    pub requested_at: String,
    pub contract_digest: ErasureDigest,
    pub catalogue_digest: ErasureDigest,
    pub policy_digest: ErasureDigest,
    pub checkpoint: ErasureEffectCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionApproval {
    pub policy_exception_digest: ErasureDigest,
    pub expires_at: String,
    pub deidentification_digest: ErasureDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequiredDisposition {
    Erase,
    ApprovedRetained { approval: RetentionApproval },
    ProvenNotApplicable { authority_evidence: ErasureDigest },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageRequirement {
    pub scope: ResidueScope,
    pub disposition: RequiredDisposition,
}

/// Supplied by the trusted catalogue/policy boundary, never inferred from observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveragePlan {
    pub version: u32,
    pub binding: VerificationBinding,
    pub requirements: Vec<CoverageRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanPage {
    pub cursor: Option<ErasureDigest>,
    pub next_cursor: Option<ErasureDigest>,
    pub remaining: u64,
    pub evidence_digest: ErasureDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteScan {
    pub started_at: String,
    pub finished_at: String,
    pub pages: Vec<ScanPage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteReason {
    PermissionDenied,
    Unavailable,
    Unsupported,
    UnknownFormat,
    UnqualifiedAbsence,
    PartialListing,
    MissingPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResidueObservation {
    ObservedZero {
        scan: CompleteScan,
    },
    Residual {
        scan: CompleteScan,
    },
    ApprovedRetained {
        scan: CompleteScan,
        approval: RetentionApproval,
    },
    ProvenNotApplicable {
        observed_at: String,
        authority_evidence: ErasureDigest,
    },
    Incomplete {
        reason: IncompleteReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeObservation {
    pub scope: ResidueScope,
    pub observation: ResidueObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageObservations {
    pub version: u32,
    pub binding: VerificationBinding,
    pub observations: Vec<ScopeObservation>,
}

#[cfg(test)]
mod compatibility_tests;
#[cfg(test)]
mod tests;
