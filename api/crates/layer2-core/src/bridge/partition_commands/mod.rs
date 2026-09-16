//! Explicit, revision-checked partition changes. A detached head remains
//! retired until an explicit reset; accepted work retains its original target.
mod service;
mod shapes;
mod write;

use super::{allocation::AllocationRecord, ingress::StoredInbox, BridgeManagementLease};
pub use service::BridgePartitionCommandService;
pub use shapes::*;
pub use write::{validate_snapshot, PartitionCommandWrite};

#[derive(Debug, thiserror::Error)]
pub enum PartitionCommandError {
    #[error(transparent)]
    Bridge(#[from] super::BridgeError),
    #[error(transparent)]
    Allocation(#[from] super::allocation::AllocationError),
    #[error("invalid partition command")]
    Invalid,
    #[error("partition not found")]
    NotFound,
    #[error("partition command conflicts with current state")]
    Conflict,
    #[error("original partition source evidence is unavailable")]
    SourceUnavailable,
    #[error("partition command unavailable")]
    Unavailable,
    #[error("partition command evidence is inconsistent")]
    Corrupt,
}

#[derive(Clone)]
pub struct PartitionCommandSnapshot {
    pub allocation: AllocationRecord,
    pub source: StoredInbox,
}

#[async_trait::async_trait]
pub trait BridgePartitionCommandStore: Send + Sync {
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        request_sha256: &str,
    ) -> Result<Option<PartitionCommandReceipt>, PartitionCommandError>;
    /// Exact current head and checked original source, never a caller assertion.
    async fn snapshot(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<PartitionCommandSnapshot, PartitionCommandError>;
    async fn apply(
        &self,
        write: PartitionCommandWrite,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError>;
}

pub fn allocation_id(id: &str) -> bool {
    id.strip_prefix("bral_").is_some_and(digest)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn command_hash(value: &impl serde::Serialize) -> Result<String, PartitionCommandError> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| PartitionCommandError::Corrupt)?)
    ))
}
