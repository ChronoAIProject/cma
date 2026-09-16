#![allow(dead_code)]
use super::super::resource_access_shapes::ResourceActorView;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PartitionCommandBody {
    pub expected_bridge_revision: u64,
    pub expected_revision: u64,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionCommandActionShape {
    Reset,
    Detach,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionCommandStateShape {
    Reserved,
    Ready,
    Retired,
}
#[derive(ToSchema)]
pub(crate) struct PartitionCommandResultShape {
    id: String,
    generation: u64,
    revision: u64,
    state: PartitionCommandStateShape,
}
#[derive(ToSchema)]
pub(crate) struct PartitionCommandReceiptShape {
    id: String,
    bridge_id: String,
    request_sha256: String,
    actor: ResourceActorView,
    action: PartitionCommandActionShape,
    expected_bridge_revision: u64,
    allocation_id: String,
    generation: u64,
    previous_revision: u64,
    result: PartitionCommandResultShape,
    successor: Option<PartitionCommandResultShape>,
    created_at_ms: i64,
}
