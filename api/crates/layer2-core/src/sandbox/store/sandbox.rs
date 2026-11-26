//! Sandbox aggregate persistence and bounded owner inventory.
use async_trait::async_trait;

use super::{Page, PageRequest, SandboxStoreError};
use crate::sandbox::{
    Sandbox, SandboxStatus, SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
};
use crate::{SandboxId, SubjectId};

#[async_trait]
pub trait SandboxStore: Send + Sync {
    async fn owner_capacity(
        &self,
        _owner: &SubjectId,
    ) -> Result<crate::sandbox::SandboxCapacitySnapshot, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "Sandbox capacity is unavailable".into(),
        ))
    }

    /// Read compatible history for this exact failed aggregate; never backfill it.
    async fn failed_lifecycle_admission(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<crate::sandbox::SandboxLifecycleAdmission>, SandboxStoreError> {
        Ok(None)
    }

    /// Original atomic retirement admission, independent of worker retries.
    async fn retirement_admitted_at(
        &self,
        _id: &SandboxId,
    ) -> Result<Option<String>, SandboxStoreError> {
        Ok(None)
    }
    async fn retained_reopen_snapshot(
        &self,
        _id: &SandboxId,
    ) -> Result<crate::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "retained reopening is unavailable",
        ))
    }

    /// Full admission evidence for the owner assessment, without writer audits.
    async fn retained_reopen_assessment_snapshot(
        &self,
        _id: &SandboxId,
        _path: crate::sandbox::RetainedReopenPath,
    ) -> Result<crate::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "retained reopening assessment is unavailable",
        ))
    }

    async fn retained_reopen_receipt(
        &self,
        _id: &SandboxId,
    ) -> Result<Option<crate::sandbox::RetainedReopenReceipt>, SandboxStoreError> {
        Ok(None)
    }

    async fn retained_reopen_replay(
        &self,
        id: &SandboxId,
    ) -> Result<Option<crate::sandbox::RetainedReopenReplay>, SandboxStoreError> {
        Ok(self.retained_reopen_receipt(id).await?.map(Into::into))
    }

    async fn retained_reopen_history(
        &self,
        id: &SandboxId,
        key_sha256: Option<&str>,
    ) -> Result<crate::sandbox::RetainedReopenHistory, SandboxStoreError> {
        let receipt = self.retained_reopen_replay(id).await?;
        Ok(crate::sandbox::RetainedReopenHistory {
            observed_parent: None,
            latest_revision: receipt.as_ref().map(|receipt| receipt.accepted.revision),
            replay: receipt.filter(|receipt| key_sha256 == Some(receipt.key_sha256.as_str())),
        })
    }

    async fn retained_suspended_reopen_snapshot(
        &self,
        _id: &SandboxId,
    ) -> Result<crate::sandbox::RetainedReopenSnapshot, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "suspended retained reopening is unavailable",
        ))
    }

    async fn admit_retained_reopen(
        &self,
        _request: crate::sandbox::RetainedReopenRequest,
    ) -> Result<crate::sandbox::RetainedReopenAccepted, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "retained reopening is unavailable",
        ))
    }

    async fn admit_suspended_reopen(
        &self,
        _request: crate::sandbox::RetainedSuspendedReopenRequest,
    ) -> Result<crate::sandbox::RetainedReopenAccepted, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "suspended retained reopening is unavailable",
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
    /// Read once per admission: each call may observe a newer release.
    fn managed_runtime_version(&self) -> Option<String> {
        None
    }

    fn desktop_runtime_target_is_qualified(&self, _target: &str) -> bool {
        false
    }

    fn managed_runtime_target<'a>(&self, sandbox: &Sandbox, target: &'a str) -> Option<&'a str> {
        (!target.is_empty()
            && (!sandbox.has_managed_desktop_image()
                || self.desktop_runtime_target_is_qualified(target)))
        .then_some(target)
    }

    fn capture_released_runtime(&self) -> Option<layer3_adapter::ReleasedRuntime> {
        self.managed_runtime_version()
            .map(|agent| layer3_adapter::ReleasedRuntime {
                revision: 0,
                agent,
                machine: None,
                lane_revisions: None,
            })
    }

    async fn refresh_released_runtime(&self) {}

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

    async fn save_sandbox_against_release(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
        _release: &layer3_adapter::ReleasedRuntime,
        _lane: crate::released_runtime::ReleasedLane,
    ) -> Result<(), SandboxStoreError> {
        self.save_sandbox(expected_revision, sandbox).await
    }

    /// Bind a default image before the first dispatch of an unpooled generation.
    async fn reserve_initial_runtime_image(
        &self,
        _sandbox: &Sandbox,
        _generation: u64,
        image: &str,
    ) -> Result<String, SandboxStoreError> {
        Ok(image.into())
    }
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
