//! Bounded collector observations. Wire data never grants effect authority.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runtime_ledger_maintenance::{valid_sha256, LedgerBindingV1};
use crate::runtime_ledger_retirement::{ledger_retired_agent_key, ledger_retired_worktree_key};

pub const COLLECTION_FORMAT: &str = "runtime-ledger-authority-request-v1";
pub const MAX_COLLECTION_GROUPS: usize = 64;
pub const MAX_COLLECTION_DEPENDENCIES: usize = 256;
pub const MAX_COLLECTION_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_COLLECTION_OBSERVATION_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CollectionGroup {
    Agent { agent_id: String },
    Worktree { workspace: String, name: String },
}

impl CollectionGroup {
    pub fn key(&self) -> String {
        match self {
            Self::Agent { agent_id } => ledger_retired_agent_key(agent_id),
            Self::Worktree { workspace, name } => ledger_retired_worktree_key(workspace, name),
        }
    }

    pub fn validate(&self) -> bool {
        match self {
            Self::Agent { agent_id } => identifier(agent_id),
            Self::Worktree { workspace, name } => identifier(workspace) && identifier(name),
        }
    }
}

/// A produces this under its operation gate. A stale revision invalidates the
/// cursor; an empty group vector is not itself the end-of-inventory marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerCandidatePageV1 {
    pub format: String,
    pub binding: LedgerBindingV1,
    pub active_revision: u64,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub groups: Vec<CollectionGroup>,
    pub complete: bool,
}

impl LedgerCandidatePageV1 {
    pub fn validate(&self) -> bool {
        self.format == "runtime-ledger-candidates-v1"
            && self.binding.validate().is_ok()
            && self.active_revision > 0
            && cursor_valid(&self.cursor)
            && cursor_valid(&self.next_cursor)
            && self.complete != self.next_cursor.is_some()
            && (self.complete || self.cursor != self.next_cursor)
            && groups_valid(&self.groups)
            && serde_json::to_vec(self).is_ok_and(|b| b.len() <= 2 * 1024 * 1024)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionRequest {
    pub format: String,
    /// Independent of the worker claim; retries retain this exact identity.
    pub operation_id: String,
    pub binding: LedgerBindingV1,
    pub active_revision: u64,
    pub page_sha256: String,
    pub groups: Vec<CollectionGroup>,
}

impl CollectionRequest {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_COLLECTION_REQUEST_BYTES {
            return None;
        }
        let value: Self = serde_json::from_slice(bytes).ok()?;
        value.validate().then_some(value)
    }

    pub fn validate(&self) -> bool {
        self.format == COLLECTION_FORMAT
            && valid_operation(&self.operation_id)
            && self.binding.validate().is_ok()
            && self.active_revision > 0
            && valid_sha256(&self.page_sha256)
            && !self.groups.is_empty()
            && groups_valid(&self.groups)
            && serde_json::to_vec(self).is_ok_and(|b| b.len() <= MAX_COLLECTION_REQUEST_BYTES)
    }

    /// Structured request identity is domain separated and length prefixed.
    /// Exact original observation bytes use a separate byte fingerprint.
    pub fn fingerprint(&self) -> Option<String> {
        if !self.validate() {
            return None;
        }
        let mut hash = Sha256::new();
        hash.update(b"chrono.runtime.ledger.collection.request.v1\0");
        let binding = self.binding.binding_sha256().ok()?;
        for bytes in [
            self.format.as_bytes(),
            self.operation_id.as_bytes(),
            self.page_sha256.as_bytes(),
            binding.as_bytes(),
        ] {
            field(&mut hash, bytes);
        }
        hash.update(self.active_revision.to_be_bytes());
        for group in &self.groups {
            field(&mut hash, group.key().as_bytes());
        }
        Some(format!("{:x}", hash.finalize()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerGroupDecisionV1 {
    SettledDeleted,
    RetainLive,
    RetainUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerResourceRevisionV1 {
    pub resource_sha256: String,
    pub revision: u64,
}

/// This is deliberately distinct from the old whole-hierarchy snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerGroupAuthorityV1 {
    pub format: String,
    pub binding: LedgerBindingV1,
    pub group_key: String,
    pub sandbox_revision: u64,
    pub resource_revisions: Vec<LedgerResourceRevisionV1>,
    pub observed_at: String,
    pub observation_sha256: String,
    pub decision: LedgerGroupDecisionV1,
    pub evidence_reference: String,
}

/// Retain these exact bytes alongside their reference. Never recanonicalize an
/// incoming artifact and call the new encoding the original evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerGroupEvidenceV1 {
    pub format: String,
    pub request_sha256: String,
    pub binding_sha256: String,
    pub group_key: String,
    pub sandbox_revision: u64,
    pub resource_revisions: Vec<LedgerResourceRevisionV1>,
    pub source_sha256: String,
    pub observed_at: String,
    pub decision: LedgerGroupDecisionV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionObservation {
    pub format: String,
    pub request_sha256: String,
    pub groups: Vec<LedgerGroupAuthorityV1>,
    pub evidence: Vec<Vec<u8>>,
    pub dependencies_read: u64,
    pub source_bytes: u64,
}

impl CollectionObservation {
    pub fn validate_for(&self, request: &CollectionRequest) -> bool {
        let Some(request_hash) = request.fingerprint() else {
            return false;
        };
        let Ok(binding_sha256) = request.binding.binding_sha256() else {
            return false;
        };
        self.format == "runtime-ledger-authority-batch-v1"
            && self.request_sha256 == request_hash
            && self.groups.len() == request.groups.len()
            && self.groups.len() == self.evidence.len()
            && self.dependencies_read > 0
            && self.dependencies_read <= MAX_COLLECTION_DEPENDENCIES as u64
            && self.source_bytes > 0
            && self.source_bytes <= 16 * 1024 * 1024
            && self
                .groups
                .iter()
                .zip(&self.evidence)
                .zip(&request.groups)
                .all(|((authority, bytes), group)| {
                    if bytes.len() > MAX_COLLECTION_OBSERVATION_BYTES {
                        return false;
                    }
                    let Ok(evidence) = serde_json::from_slice::<LedgerGroupEvidenceV1>(bytes)
                    else {
                        return false;
                    };
                    let hash = format!("{:x}", Sha256::digest(bytes));
                    authority.format == "runtime-ledger-group-authority-v1"
                        && authority.binding.validate_for(&request.binding).is_ok()
                        && authority.group_key == group.key()
                        && authority.sandbox_revision == request.binding.sandbox_revision
                        && authority.observation_sha256 == hash
                        && authority.evidence_reference == format!("sha256:{hash}")
                        && evidence.format == "runtime-ledger-group-evidence-v1"
                        && evidence.request_sha256 == request_hash
                        && evidence.binding_sha256 == binding_sha256
                        && evidence.group_key == authority.group_key
                        && evidence.sandbox_revision == authority.sandbox_revision
                        && evidence.resource_revisions == authority.resource_revisions
                        && evidence.observed_at == authority.observed_at
                        && evidence.decision == authority.decision
                        && valid_sha256(&evidence.source_sha256)
                        && time::OffsetDateTime::parse(
                            &evidence.observed_at,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .is_ok()
                        && revisions_valid(&evidence.resource_revisions)
                })
            && serde_json::to_vec(self).is_ok_and(|b| b.len() <= MAX_COLLECTION_OBSERVATION_BYTES)
    }
}

fn valid_operation(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn groups_valid(groups: &[CollectionGroup]) -> bool {
    groups.len() <= MAX_COLLECTION_GROUPS
        && groups.iter().all(CollectionGroup::validate)
        && groups.windows(2).all(|p| p[0].key() < p[1].key())
}
fn revisions_valid(revisions: &[LedgerResourceRevisionV1]) -> bool {
    !revisions.is_empty()
        && revisions.len() <= MAX_COLLECTION_DEPENDENCIES
        && revisions
            .iter()
            .all(|r| r.revision > 0 && valid_sha256(&r.resource_sha256))
        && revisions
            .windows(2)
            .all(|p| p[0].resource_sha256 < p[1].resource_sha256)
}
fn cursor_valid(cursor: &Option<String>) -> bool {
    cursor
        .as_ref()
        .is_none_or(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

#[cfg(test)]
#[path = "runtime_ledger_collection_tests.rs"]
mod tests;
