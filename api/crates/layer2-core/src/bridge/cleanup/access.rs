use super::{allocation_id, ChildCleanupRequest, ChildCleanupStatus, CleanupError};
use crate::{
    bridge::{allocation::AllocationRecord, BridgeManagementLease},
    resource_access::{AuthorizedResource, ResourceCapability, ResourceTarget},
};

/// Current management/read boundary. Runtime readiness is deliberately absent
/// so terminal cleanup remains readable after a parent pauses or loses custody.
pub struct CleanupAccess {
    pub(super) management: BridgeManagementLease,
    pub(super) parent: AuthorizedResource,
    pub(super) allocation: AllocationRecord,
}

impl CleanupAccess {
    pub fn management(&self) -> &BridgeManagementLease {
        &self.management
    }
    pub fn parent(&self) -> &crate::resource_access::ResourceAccessSnapshot {
        &self.parent.snapshot
    }
    pub fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub fn require_current(&self) -> Result<(), CleanupError> {
        self.management.require_current()?;
        self.parent
            .decision
            .require(ResourceCapability::ManageSandbox)
            .map_err(CleanupError::from)?;
        self.allocation
            .validate()
            .map_err(|_| CleanupError::Corrupt)?;
        let record = self.management.record();
        let sandbox = &self.parent.snapshot.lineage.sandbox;
        let scope = self
            .parent
            .snapshot
            .scope(&ResourceTarget::Sandbox {
                sandbox_id: sandbox.id.clone(),
            })
            .map_err(|_| CleanupError::Corrupt)?;
        if self.allocation.identity.bridge_id != record.id
            || self.allocation.owner != record.owner
            || self.allocation.placement.target().sandbox_id != sandbox.id
            || scope.owner != record.owner
            || record.pinned(&self.allocation.pin)?.config != self.allocation.config
        {
            return Err(CleanupError::NotFound);
        }
        Ok(())
    }
    pub fn require_status(&self, status: &ChildCleanupStatus) -> Result<(), CleanupError> {
        self.require_current()?;
        let allocation = &self.allocation;
        if status.bridge_id != allocation.identity.bridge_id
            || status.allocation_id != allocation.id
            || status.generation != allocation.identity.generation
            || status.allocation_revision != allocation.revision
            || &status.target != allocation.placement.target()
            || !status.workspace_retained
            || !status.history_retained
            || status.holds.windows(2).any(|pair| pair[0] >= pair[1])
            || status.observed_at_ms.is_some_and(|time| time < 0)
            || (status.progress == super::ChildCleanupProgress::NotRequested)
                != status.receipt.is_none()
            || status.agent_capacity_released
                != (status.progress == super::ChildCleanupProgress::Complete)
            || (status.receipt.is_some() && status.observed_at_ms.is_none())
            || (status.receipt.is_none()
                && (!status.holds.is_empty() || status.observed_at_ms.is_some()))
            || (status.progress == super::ChildCleanupProgress::Complete
                && !status.holds.is_empty())
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
                || allocation
                    .placement
                    .profiles()
                    .is_none_or(|profiles| profiles.approved_by != receipt.requested_by)
                || !receipt.id.strip_prefix("bclj_").is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
            {
                return Err(CleanupError::Corrupt);
            }
        }
        Ok(())
    }
}

/// An authenticated explicit intent. The store still resolves current original
/// configuring-actor custody and fences the exact effect scope transactionally.
pub struct CleanupWrite {
    pub(super) access: CleanupAccess,
    pub(super) request: ChildCleanupRequest,
    pub(super) receipt_id: String,
    pub(super) request_sha256: String,
    pub(super) issuer: String,
}
impl CleanupWrite {
    pub fn access(&self) -> &CleanupAccess {
        &self.access
    }
    pub fn request(&self) -> &ChildCleanupRequest {
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
        if self.access.management.record().revision != self.request.expected_bridge_revision
            || self.access.allocation.revision != self.request.expected_allocation_revision
        {
            return Err(CleanupError::Conflict);
        }
        let original = self
            .access
            .allocation
            .placement
            .profiles()
            .ok_or(CleanupError::OriginalActorRequired)?;
        if self.access.management.actor() != &original.approved_by {
            return Err(CleanupError::OriginalActorRequired);
        }
        Ok(())
    }
}
