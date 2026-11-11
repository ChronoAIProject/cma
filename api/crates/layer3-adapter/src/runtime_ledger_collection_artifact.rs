//! An immutable locator for retained original collection bytes. Validation is
//! structural only: neither this reference nor its hashes grant maintenance.
use serde::{Deserialize, Serialize};

pub const LEDGER_COLLECTION_ARTIFACT_FORMAT: &str = "runtime-ledger-collection-reference-v1";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCollectionArtifactReferenceV1 {
    pub format: String,
    pub archive_id: String,
    pub archive_version: u8,
    pub operation_id: String,
    pub group_key: String,
    pub binding_sha256: String,
    pub plan_sha256: String,
    pub artifacts_sha256: String,
    /// Candidate, request and observation lengths, in this order.
    pub artifact_lengths: [u64; 3],
}

impl LedgerCollectionArtifactReferenceV1 {
    pub fn validate(&self) -> bool {
        self.format == LEDGER_COLLECTION_ARTIFACT_FORMAT
            && self.archive_version == 3
            && [&self.archive_id, &self.operation_id]
                .into_iter()
                .all(|value| {
                    uuid::Uuid::parse_str(value)
                        .is_ok_and(|id| !id.is_nil() && id.to_string() == *value)
                })
            && [
                &self.group_key,
                &self.binding_sha256,
                &self.plan_sha256,
                &self.artifacts_sha256,
            ]
            .into_iter()
            .all(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            && self
                .artifact_lengths
                .iter()
                .zip([2 * 1024 * 1024, 65_536, 4 * 1024 * 1024])
                .all(|(length, limit)| (1..=limit).contains(length))
    }
}

// Operational diagnostics expose only fixed labels, hashes and byte counts.
impl std::fmt::Debug for LedgerCollectionArtifactReferenceV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerCollectionArtifactReferenceV1")
            .field("binding_sha256", &self.binding_sha256)
            .field("group_key", &self.group_key)
            .field("plan_sha256", &self.plan_sha256)
            .field("artifacts_sha256", &self.artifacts_sha256)
            .field("artifact_lengths", &self.artifact_lengths)
            .finish_non_exhaustive()
    }
}
