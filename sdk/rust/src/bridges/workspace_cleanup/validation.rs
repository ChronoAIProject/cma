use super::super::{BridgeAgentTarget, validation as bridge};
use super::*;
use crate::resource_access::ResourceActor;

fn prefixed(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub(super) fn allocation_id(value: &str) -> Result<()> {
    prefixed(value, "bral_")
        .then_some(())
        .ok_or_else(|| Error::validation("invalid allocation identity"))
}
fn target(value: &BridgeAgentTarget) -> bool {
    [
        &value.sandbox_id,
        &value.workspace_id,
        &value.worktree_id,
        &value.agent_id,
    ]
    .iter()
    .all(|id| bridge::valid_id(id))
}
fn revision(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}
pub(super) fn request(value: &BridgeWorkspaceCleanupRequest) -> Result<()> {
    if !revision(value.expected_bridge_revision)
        || !revision(value.expected_allocation_revision)
        || !revision(value.expected_workspace_revision)
        || !prefixed(&value.child_cleanup_receipt_id, "bclj_")
    {
        return Err(Error::validation(
            "invalid Workspace cleanup revisions or child receipt",
        ));
    }
    Ok(())
}
fn hold_rank(value: BridgeWorkspaceCleanupHold) -> usize {
    use BridgeWorkspaceCleanupHold::*;
    match value {
        AllocationNotRetired => 0,
        WorkspaceNotOwned => 1,
        ChildCleanupIncomplete => 2,
        WorkspaceBusy => 3,
        OtherWorkspaceChildren => 4,
        InventoryLimit => 5,
        AuthorityUnavailable => 6,
        ParentUnavailable => 7,
        RuntimeUnsupported => 8,
        RuntimeChanged => 9,
        EvidenceUnavailable => 10,
        EvidenceInconsistent => 11,
    }
}
impl BridgeWorkspaceCleanupStatus {
    pub(super) fn check(
        &self,
        bridge_id: &str,
        allocation_id: &str,
        request: Option<&BridgeWorkspaceCleanupRequest>,
    ) -> Result<()> {
        if self.bridge_id != bridge_id
            || self.allocation_id != allocation_id
            || !prefixed(&self.allocation_id, "bral_")
            || !(1..=256).contains(&self.generation)
            || !revision(self.allocation_revision)
            || !target(&self.target)
            || !self.parent_retained
            || !self.history_retained
            || self.workspace_capacity_released
                != (self.progress == BridgeWorkspaceCleanupProgress::Complete)
            || (self.progress == BridgeWorkspaceCleanupProgress::NotRequested)
                != self.receipt.is_none()
            || self
                .workspace_revision
                .is_some_and(|value| !revision(value))
            || self
                .child_cleanup_receipt_id
                .as_deref()
                .is_some_and(|id| !prefixed(id, "bclj_"))
            || (self.eligible
                && (!self.runtime_supported
                    || !self.holds.is_empty()
                    || self.receipt.is_some()
                    || self.workspace_revision.is_none()
                    || self.child_cleanup_receipt_id.is_none()))
            || (self.receipt.is_some()
                && (self.observed_at_ms.is_none() || self.workspace_revision.is_none()))
            || self.observed_at_ms.is_some_and(|at| at > i64::MAX as u64)
            || (self.progress == BridgeWorkspaceCleanupProgress::Complete && !self.holds.is_empty())
            || self
                .holds
                .windows(2)
                .any(|pair| hold_rank(pair[0]) >= hold_rank(pair[1]))
        {
            return Err(Error::protocol(
                "Workspace cleanup identity, target or progress is inconsistent",
            ));
        }
        if let Some(receipt) = &self.receipt {
            if !prefixed(&receipt.id, "bwcl_")
                || receipt.bridge_id != self.bridge_id
                || receipt.allocation_id != self.allocation_id
                || receipt.generation != self.generation
                || receipt.target != self.target
                || receipt.accepted_at_ms > i64::MAX as u64
                || super::validation::request(&receipt.request).is_err()
                || self.child_cleanup_receipt_id.as_ref()
                    != Some(&receipt.request.child_cleanup_receipt_id)
                || receipt.request.expected_allocation_revision != self.allocation_revision
                || !matches!(&receipt.requested_by, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if bridge::valid_id(subject))
                || request.is_some_and(|request| request != &receipt.request)
            {
                return Err(Error::protocol(
                    "Workspace cleanup receipt does not match its exact request and target",
                ));
            }
        } else if request.is_some() {
            return Err(Error::protocol(
                "Workspace cleanup POST returned no acceptance receipt",
            ));
        }
        Ok(())
    }
}
