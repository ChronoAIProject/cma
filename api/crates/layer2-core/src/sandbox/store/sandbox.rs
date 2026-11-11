//! Sandbox aggregate persistence and bounded owner inventory.
use async_trait::async_trait;

use super::{Page, PageRequest, SandboxStoreError};
use crate::sandbox::{
    Sandbox, SandboxStatus, SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
};
use crate::{SandboxId, SubjectId};

#[async_trait]
pub trait SandboxStore: Send + Sync {
    async fn retained_reopen_snapshot(
        &self,
        _id: &SandboxId,
    ) -> Result<crate::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "retained reopening is unavailable",
        ))
    }

    async fn retained_reopen_receipt(
        &self,
        _id: &SandboxId,
    ) -> Result<Option<crate::sandbox::RetainedReopenReceipt>, SandboxStoreError> {
        Ok(None)
    }

    async fn admit_retained_reopen(
        &self,
        _request: crate::sandbox::RetainedReopenRequest,
    ) -> Result<crate::sandbox::RetainedReopenAccepted, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "retained reopening is unavailable",
        ))
    }

    /// Read-only complete retained recovery evidence, including published logs.
    /// This endpoint never grants the unpublished admission's stronger predicate.
    async fn retained_runtime_recovery_snapshot(
        &self,
        _id: &SandboxId,
    ) -> Result<crate::sandbox::RetainedRecoverySnapshot, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "retained recovery inspection is unavailable",
        ))
    }

    /// Current server-managed runtime; custom images and lost-runtime recovery stay pinned.
    fn managed_runtime_version(&self) -> Option<&str> {
        None
    }

    async fn unpublished_runtime_recovery_snapshot(
        &self,
        _id: &SandboxId,
    ) -> Result<SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "unpublished runtime recovery is unavailable",
        ))
    }
    async fn admit_unpublished_runtime_recovery(
        &self,
        _request: SandboxUnpublishedRecoveryRequest,
    ) -> Result<Sandbox, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "unpublished runtime recovery is unavailable",
        ))
    }
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError>;
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError>;
    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError>;
    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError>;

    /// Owner Sandboxes excluding Retiring and Retired. Persistent stores can
    /// filter before paging to avoid decoding retained retirement history.
    /// The fallback preserves the underlying cursor, including empty pages;
    /// callers must follow `next` until it is absent.
    async fn list_live_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        let mut result = self.list_sandboxes(owner, page).await?;
        result.items.retain(|sandbox| {
            !matches!(
                sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
        });
        Ok(result)
    }
}
