use super::*;

impl CleanupAccess {
    pub fn require_workspace_status(
        &self,
        status: &WorkspaceCleanupStatus,
    ) -> Result<(), CleanupError> {
        self.require_current()?;
        let allocation = self.allocation();
        if status.bridge_id != allocation.identity.bridge_id
            || status.allocation_id != allocation.id
            || status.generation != allocation.identity.generation
            || status.allocation_revision != allocation.revision
            || &status.target != allocation.placement.target()
            || !status.parent_retained
            || !status.history_retained
            || status.holds.windows(2).any(|pair| pair[0] >= pair[1])
            || status.observed_at_ms.is_some_and(|time| time < 0)
            || status
                .workspace_revision
                .is_some_and(|revision| revision == 0 || revision > i64::MAX as u64)
            || (status.progress == WorkspaceCleanupProgress::NotRequested)
                != status.receipt.is_none()
            || status.workspace_capacity_released
                != (status.progress == WorkspaceCleanupProgress::Complete)
            || (status.eligible
                && (!status.runtime_supported
                    || status.workspace_revision.is_none()
                    || status.child_cleanup_receipt_id.is_none()
                    || !status.holds.is_empty()
                    || status.receipt.is_some()))
            || (status.receipt.is_some()
                && (status.observed_at_ms.is_none() || status.workspace_revision.is_none()))
            || (status.progress == WorkspaceCleanupProgress::Complete && !status.holds.is_empty())
            || status
                .child_cleanup_receipt_id
                .as_ref()
                .is_some_and(|id| !id.strip_prefix("bclj_").is_some_and(workspace_hash))
        {
            return Err(CleanupError::Corrupt);
        }
        if let Some(receipt) = &status.receipt {
            receipt
                .request
                .validate()
                .map_err(|_| CleanupError::Corrupt)?;
            if receipt.bridge_id != status.bridge_id
                || receipt.allocation_id != status.allocation_id
                || receipt.generation != status.generation
                || receipt.target != status.target
                || !allocation_id(&receipt.allocation_id)
                || receipt.accepted_at_ms < 0
                || receipt.request.expected_allocation_revision != status.allocation_revision
                || status.child_cleanup_receipt_id.as_ref()
                    != Some(&receipt.request.child_cleanup_receipt_id)
                || allocation
                    .placement
                    .profiles()
                    .is_none_or(|profiles| profiles.approved_by != receipt.requested_by)
                || !receipt.id.strip_prefix("bwcl_").is_some_and(workspace_hash)
            {
                return Err(CleanupError::Corrupt);
            }
        }
        Ok(())
    }
}

pub struct WorkspaceCleanupWrite {
    pub(super) access: CleanupAccess,
    pub(super) request: WorkspaceCleanupRequest,
    pub(super) receipt_id: String,
    pub(super) request_sha256: String,
    pub(super) issuer: String,
}
impl WorkspaceCleanupWrite {
    pub fn access(&self) -> &CleanupAccess {
        &self.access
    }
    pub fn request(&self) -> &WorkspaceCleanupRequest {
        &self.request
    }
    pub fn receipt_id(&self) -> &str {
        &self.receipt_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn require_current(&self) -> Result<(), CleanupError> {
        self.access.require_current()?;
        self.request.validate()?;
        if self.access.management().record().revision != self.request.expected_bridge_revision
            || self.access.allocation().revision != self.request.expected_allocation_revision
        {
            return Err(CleanupError::Conflict);
        }
        let original = self
            .access
            .allocation()
            .placement
            .profiles()
            .ok_or(CleanupError::OriginalActorRequired)?;
        if self.access.management().actor() != &original.approved_by {
            return Err(CleanupError::OriginalActorRequired);
        }
        Ok(())
    }
}
