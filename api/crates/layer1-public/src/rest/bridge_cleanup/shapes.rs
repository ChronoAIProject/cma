#![allow(dead_code)]
use super::super::resource_access_shapes::ResourceActorView;
use utoipa::ToSchema;

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChildCleanupActionShape {
    RemoveChild,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChildCleanupRequestShape {
    expected_bridge_revision: u64,
    expected_allocation_revision: u64,
    action: ChildCleanupActionShape,
}
#[derive(ToSchema)]
pub(crate) struct CleanupTargetShape {
    sandbox_id: String,
    workspace_id: String,
    worktree_id: String,
    agent_id: String,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChildCleanupReceiptShape {
    id: String,
    bridge_id: String,
    allocation_id: String,
    generation: u64,
    target: CleanupTargetShape,
    request: ChildCleanupRequestShape,
    requested_by: ResourceActorView,
    accepted_at_ms: i64,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChildCleanupProgressShape {
    NotRequested,
    Requested,
    AdmissionClosed,
    RemovalStarted,
    ProcessClosed,
    HomeRemoved,
    WorktreeRemoved,
    Complete,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChildCleanupHoldShape {
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
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChildCleanupStatusShape {
    bridge_id: String,
    allocation_id: String,
    generation: u64,
    allocation_revision: u64,
    target: CleanupTargetShape,
    receipt: Option<ChildCleanupReceiptShape>,
    progress: ChildCleanupProgressShape,
    holds: Vec<ChildCleanupHoldShape>,
    observed_at_ms: Option<i64>,
    agent_capacity_released: bool,
    workspace_retained: bool,
    history_retained: bool,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanupPlacementShape {
    ExistingAgent,
    NewSandbox,
    ReuseParent,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanupAllocationStateShape {
    Reserved,
    Provisioning,
    Ready,
    NeedsConnection,
    FailedRetryable,
    FailedTerminal,
    Retired,
}
#[derive(ToSchema)]
pub(crate) struct CleanupAllocationShape {
    id: String,
    bridge_id: String,
    generation: u64,
    revision: u64,
    target: CleanupTargetShape,
    placement: CleanupPlacementShape,
    state: CleanupAllocationStateShape,
}
#[derive(ToSchema)]
pub(crate) struct CleanupAllocationPageShape {
    items: Vec<CleanupAllocationShape>,
    next_cursor: Option<String>,
}
