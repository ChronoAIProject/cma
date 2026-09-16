//! Separate consent for removing the Workspace originally created by one
//! retired allocation. The child receipt retains its original effect scope.
use crate::{
    bridge::{BridgeAgentTarget, BridgeId},
    resource_access::ResourceActor,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCleanupAction {
    RemoveOwnedWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupRequest {
    pub expected_bridge_revision: u64,
    pub expected_allocation_revision: u64,
    pub expected_workspace_revision: u64,
    pub child_cleanup_receipt_id: String,
    pub action: WorkspaceCleanupAction,
}
impl WorkspaceCleanupRequest {
    pub fn validate(&self) -> Result<(), super::CleanupError> {
        if [
            self.expected_bridge_revision,
            self.expected_allocation_revision,
            self.expected_workspace_revision,
        ]
        .iter()
        .any(|revision| *revision == 0 || *revision > i64::MAX as u64)
            || !self
                .child_cleanup_receipt_id
                .strip_prefix("bclj_")
                .is_some_and(super::workspace_hash)
        {
            return Err(super::CleanupError::Invalid);
        }
        Ok(())
    }
}

pub struct WorkspaceCleanupCommand {
    pub allocation_id: String,
    pub idempotency_key: String,
    pub request: WorkspaceCleanupRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupReceipt {
    pub id: String,
    pub bridge_id: BridgeId,
    pub allocation_id: String,
    pub generation: u64,
    pub target: BridgeAgentTarget,
    pub request: WorkspaceCleanupRequest,
    pub requested_by: ResourceActor,
    pub accepted_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCleanupProgress {
    NotRequested,
    Requested,
    AdmissionClosed,
    RemovalStarted,
    RuntimeRemoved,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCleanupHold {
    AllocationNotRetired,
    WorkspaceNotOwned,
    ChildCleanupIncomplete,
    WorkspaceBusy,
    OtherWorkspaceChildren,
    InventoryLimit,
    AuthorityUnavailable,
    ParentUnavailable,
    RuntimeUnsupported,
    RuntimeChanged,
    EvidenceUnavailable,
    EvidenceInconsistent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupStatus {
    pub bridge_id: BridgeId,
    pub allocation_id: String,
    pub generation: u64,
    pub allocation_revision: u64,
    pub target: BridgeAgentTarget,
    pub workspace_revision: Option<u64>,
    pub child_cleanup_receipt_id: Option<String>,
    pub eligible: bool,
    pub runtime_supported: bool,
    pub receipt: Option<WorkspaceCleanupReceipt>,
    pub progress: WorkspaceCleanupProgress,
    pub holds: Vec<WorkspaceCleanupHold>,
    pub observed_at_ms: Option<i64>,
    pub workspace_capacity_released: bool,
    pub parent_retained: bool,
    pub history_retained: bool,
}
