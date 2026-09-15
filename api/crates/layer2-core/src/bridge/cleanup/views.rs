use crate::{
    bridge::{allocation::AllocationState, BridgeAgentTarget, BridgeId},
    resource_access::ResourceActor,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildCleanupAction {
    RemoveChild,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildCleanupRequest {
    pub expected_bridge_revision: u64,
    pub expected_allocation_revision: u64,
    pub action: ChildCleanupAction,
}

/// Internal service arguments; the transport carries the allocation in its
/// route and the retained key in its existing idempotency header.
pub struct ChildCleanupCommand {
    pub allocation_id: String,
    pub idempotency_key: String,
    pub request: ChildCleanupRequest,
}
impl ChildCleanupRequest {
    pub fn validate(&self) -> Result<(), super::CleanupError> {
        if [
            self.expected_bridge_revision,
            self.expected_allocation_revision,
        ]
        .iter()
        .any(|revision| *revision == 0 || *revision > i64::MAX as u64)
        {
            return Err(super::CleanupError::Invalid);
        }
        Ok(())
    }
}

/// Immutable acceptance. Current progress is a separate observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildCleanupReceipt {
    pub id: String,
    pub bridge_id: BridgeId,
    pub allocation_id: String,
    pub generation: u64,
    pub target: BridgeAgentTarget,
    pub request: ChildCleanupRequest,
    pub requested_by: ResourceActor,
    pub accepted_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildCleanupProgress {
    NotRequested,
    Requested,
    AdmissionClosed,
    RemovalStarted,
    ProcessClosed,
    HomeRemoved,
    WorktreeRemoved,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildCleanupHold {
    AgentBusy,
    WorkspaceBusy,
    PendingCommand,
    UncertainCommand,
    PendingReply,
    UncertainReply,
    OtherWorkspaceChildren,
    InventoryLimit,
    AuthorityUnavailable,
    ParentUnavailable,
    RuntimeChanged,
    EvidenceUnavailable,
    EvidenceInconsistent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildCleanupStatus {
    pub bridge_id: BridgeId,
    pub allocation_id: String,
    pub generation: u64,
    pub allocation_revision: u64,
    pub target: BridgeAgentTarget,
    pub receipt: Option<ChildCleanupReceipt>,
    pub progress: ChildCleanupProgress,
    pub holds: Vec<ChildCleanupHold>,
    pub observed_at_ms: Option<i64>,
    pub agent_capacity_released: bool,
    pub workspace_retained: bool,
    pub history_retained: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPlacement {
    ExistingAgent,
    NewSandbox,
    ReuseParent,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupAllocationView {
    pub id: String,
    pub bridge_id: BridgeId,
    pub generation: u64,
    pub revision: u64,
    pub target: BridgeAgentTarget,
    pub placement: CleanupPlacement,
    pub state: AllocationState,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupAllocationPage {
    pub items: Vec<CleanupAllocationView>,
    pub next_cursor: Option<String>,
}
