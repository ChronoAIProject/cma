#![allow(dead_code)]
use super::super::resource_access_shapes::ResourceActorView;
use super::shapes::CleanupTargetShape;
use utoipa::ToSchema;

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceCleanupActionShape {
    RemoveOwnedWorkspace,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceCleanupRequestShape {
    #[schema(minimum = 1, maximum = 9223372036854775807_i64)]
    expected_bridge_revision: u64,
    #[schema(minimum = 1, maximum = 9223372036854775807_i64)]
    expected_allocation_revision: u64,
    #[schema(minimum = 1, maximum = 9223372036854775807_i64)]
    expected_workspace_revision: u64,
    child_cleanup_receipt_id: String,
    action: WorkspaceCleanupActionShape,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceCleanupReceiptShape {
    id: String,
    bridge_id: String,
    allocation_id: String,
    generation: u64,
    target: CleanupTargetShape,
    request: WorkspaceCleanupRequestShape,
    requested_by: ResourceActorView,
    accepted_at_ms: i64,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceCleanupProgressShape {
    NotRequested,
    Requested,
    AdmissionClosed,
    RemovalStarted,
    RuntimeRemoved,
    Complete,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceCleanupHoldShape {
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
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceCleanupStatusShape {
    bridge_id: String,
    allocation_id: String,
    generation: u64,
    allocation_revision: u64,
    target: CleanupTargetShape,
    #[schema(required = true)]
    workspace_revision: Option<u64>,
    #[schema(required = true)]
    child_cleanup_receipt_id: Option<String>,
    eligible: bool,
    runtime_supported: bool,
    #[schema(required = true)]
    receipt: Option<WorkspaceCleanupReceiptShape>,
    progress: WorkspaceCleanupProgressShape,
    holds: Vec<WorkspaceCleanupHoldShape>,
    #[schema(required = true)]
    observed_at_ms: Option<i64>,
    workspace_capacity_released: bool,
    parent_retained: bool,
    history_retained: bool,
}
