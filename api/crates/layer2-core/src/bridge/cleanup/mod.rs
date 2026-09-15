//! Explicit management of one retained allocation's child. Receipts and read
//! observations never grant runtime authority or authorize parent removal.
mod access;
mod authority;
mod errors;
mod job;
mod pagination;
mod service;
mod views;
mod workspace_access;
mod workspace_job;
mod workspace_service;
mod workspace_views;

use crate::bridge::{allocation::AllocationRecord, BridgeError, BridgeManagementLease};
pub use access::{CleanupAccess, CleanupWrite};
pub use job::*;
pub use service::BridgeCleanupService;
pub use views::*;
pub use workspace_access::WorkspaceCleanupWrite;
pub use workspace_job::*;
pub use workspace_views::*;

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error("invalid child cleanup request")]
    Invalid,
    #[error("child cleanup was not found")]
    NotFound,
    #[error("child cleanup changed; retain the original request for recovery")]
    Conflict,
    #[error("cleanup requires the current authority of its original configuring actor")]
    OriginalActorRequired,
    #[error("child cleanup is unavailable")]
    Unavailable,
    #[error("retained child cleanup evidence is inconsistent")]
    Corrupt,
}

/// Composition resolves renewable original-actor authority and performs all
/// exact ownership/lineage and job/audit checks in its transaction. A public
/// request cannot directly invoke a runtime operation through this port.
#[async_trait::async_trait]
pub trait BridgeCleanupStore: Send + Sync {
    async fn allocations(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AllocationRecord>, CleanupError>;
    async fn allocation(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<AllocationRecord>, CleanupError>;
    async fn status(&self, access: &CleanupAccess) -> Result<ChildCleanupStatus, CleanupError>;
    async fn recover(
        &self,
        access: &CleanupAccess,
        receipt_id: &str,
        request_sha256: &str,
    ) -> Result<Option<ChildCleanupStatus>, CleanupError>;
    async fn submit(&self, write: CleanupWrite) -> Result<ChildCleanupStatus, CleanupError>;
    async fn workspace_status(
        &self,
        _access: &CleanupAccess,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        Err(CleanupError::Unavailable)
    }
    async fn workspace_recover(
        &self,
        _access: &CleanupAccess,
        _receipt_id: &str,
        _request_sha256: &str,
    ) -> Result<Option<WorkspaceCleanupStatus>, CleanupError> {
        Err(CleanupError::Unavailable)
    }
    async fn workspace_submit(
        &self,
        _write: WorkspaceCleanupWrite,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        Err(CleanupError::Unavailable)
    }
}

fn workspace_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn digest(value: &impl serde::Serialize) -> Result<String, CleanupError> {
    crate::profiles::content_digest(value).map_err(|_| CleanupError::Corrupt)
}

fn allocation_id(value: &str) -> bool {
    value.strip_prefix("bral_").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests;
