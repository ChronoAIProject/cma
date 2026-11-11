use super::{Result, REFUSED};
use layer3_adapter::{
    archive_source::Entry,
    permanent_custody::{digest, CustodyPolicy, ObjectReceipt},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Root {
    pub role: String,
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub format: u32,
    pub operation_id: String,
    pub sandbox_id: String,
    pub expected_revision: u64,
    pub source_sha256: String,
    pub retirement_intent_sha256: String,
    pub home_identity: String,
    pub cohort_sha256: String,
    pub epoch: u64,
    pub roots: Vec<Root>,
    pub exclusion_receipt: PathBuf,
    pub exclusion_sha256: String,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        let id = uuid::Uuid::parse_str(&self.operation_id).map_err(|_| REFUSED)?;
        if self.format != 1
            || id.is_nil()
            || id.to_string() != self.operation_id
            || self.expected_revision == 0
            || self.epoch == 0
            || !identity(&self.sandbox_id)
            || !identity(&self.home_identity)
            || ![
                &self.source_sha256,
                &self.retirement_intent_sha256,
                &self.cohort_sha256,
                &self.exclusion_sha256,
            ]
            .into_iter()
            .all(|h| hash(h))
            || self.roots.len() < 3
            || self.roots.len() > 16
            || !self.exclusion_receipt.is_absolute()
        {
            return Err(REFUSED);
        }
        let mut roles = std::collections::BTreeSet::new();
        for root in &self.roots {
            if !["home", "state", "transfer", "attachments"].contains(&root.role.as_str())
                || !roles.insert(root.role.as_str())
                || !root.path.is_absolute()
                || root.inode == 0
            {
                return Err(REFUSED);
            }
        }
        if !["home", "state", "transfer"]
            .into_iter()
            .all(|r| roles.contains(r))
        {
            return Err(REFUSED);
        }
        Ok(())
    }
    pub fn roots_sha256(&self) -> Result<String> {
        Ok(digest(
            &serde_json::to_vec(&self.roots).map_err(|_| REFUSED)?,
        ))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExclusionReceipt {
    pub format: u32,
    pub operation_id: String,
    pub sandbox_id: String,
    pub source_sha256: String,
    pub cohort_sha256: String,
    pub home_identity: String,
    pub roots_sha256: String,
    pub epoch: u64,
    pub qualified_at: String,
    pub expires_at: String,
    /// These identify release-owned provider/finality rehearsals; the command
    /// does not claim to implement or infer physical exclusion from their hashes.
    pub writer_matrix_sha256: String,
    pub provider_finality_sha256: String,
    pub physical_fence_sha256: String,
    pub retirement_intent_sha256: String,
}
impl ExclusionReceipt {
    pub fn require(&self, request: &Request, now: &str) -> Result<()> {
        let at = layer2_core::seconds_from_rfc3339(&self.qualified_at).ok_or(REFUSED)?;
        let end = layer2_core::seconds_from_rfc3339(&self.expires_at).ok_or(REFUSED)?;
        let now = layer2_core::seconds_from_rfc3339(now).ok_or(REFUSED)?;
        if self.format != 1
            || self.operation_id != request.operation_id
            || self.sandbox_id != request.sandbox_id
            || self.source_sha256 != request.source_sha256
            || self.cohort_sha256 != request.cohort_sha256
            || self.home_identity != request.home_identity
            || self.roots_sha256 != request.roots_sha256()?
            || self.retirement_intent_sha256 != request.retirement_intent_sha256
            || self.epoch != request.epoch
            || at > now
            || end <= now
            || end - at > 86400
            || ![
                &self.writer_matrix_sha256,
                &self.provider_finality_sha256,
                &self.physical_fence_sha256,
            ]
            .into_iter()
            .all(|h| hash(h))
        {
            return Err(REFUSED);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileRecord {
    pub role: String,
    pub entry: Entry,
    pub sha256: String,
    pub objects: Vec<ObjectReceipt>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseRecord {
    pub collection: String,
    pub documents: u64,
    pub bytes: u64,
    pub sha256: String,
    pub objects: Vec<ObjectReceipt>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub format: u32,
    pub operation_id: String,
    pub request_sha256: String,
    pub original_request: ObjectReceipt,
    pub original_source: ObjectReceipt,
    pub exclusion: ObjectReceipt,
    pub custody: CustodyPolicy,
    pub schema_sha256: String,
    pub external: Vec<ExternalRecord>,
    pub files: Vec<FileRecord>,
    pub database: Vec<DatabaseRecord>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Verification {
    pub format: u32,
    pub operation_id: String,
    pub manifest: ObjectReceipt,
    pub custody_sha256: String,
    pub objects: u64,
    pub files: u64,
    pub documents: u64,
    pub bytes: u64,
    pub restore_sha256: String,
}
pub(crate) fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalRecord {
    pub key: String,
    pub sha256: String,
    pub object: ObjectReceipt,
}
