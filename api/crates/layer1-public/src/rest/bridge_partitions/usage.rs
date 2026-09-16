use super::*;
use utoipa::ToSchema;

#[utoipa::path(get,path="/api/v1/bridges/{id}/usage",tag="bridges",params(("id"=String,Path)),responses((status=200,body=BridgeUsageShape),(status=403,body=super::super::ProblemDocument),(status=404,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn read(
    State(service): State<Option<Arc<BridgePartitionService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = service else {
        return problem(PartitionStatusError::Unavailable);
    };
    respond(
        service
            .usage(&caller.context, credential(&caller), &BridgeId(id))
            .await,
    )
}

#[allow(dead_code)]
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
struct BridgeUsageShape {
    bridge_id: String,
    bridge_revision: u64,
    observed_at_ms: i64,
    inventory_limit: u32,
    limits: BridgeUsageLimitsShape,
    #[schema(required = true)]
    allocations: Option<BridgeAllocationUsageShape>,
    target_resources: BridgeResourceUsageShape,
    capacity_reasons: Vec<BridgeCapacityReasonShape>,
}
#[allow(dead_code)]
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
struct BridgeUsageLimitsShape {
    max_partitions: u32,
    max_sandbox_reservations: u32,
}
#[allow(dead_code)]
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
struct BridgeAllocationUsageShape {
    retained: u32,
    current: u32,
    ready: u32,
    retired: u32,
    sandbox_reservations: u32,
}
#[allow(dead_code)]
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
struct BridgeResourceCountShape {
    retained: u32,
    active: u32,
}
#[allow(dead_code)]
#[derive(ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum BridgeResourceUsageShape {
    Available {
        sandboxes: BridgeResourceCountShape,
        workspaces: BridgeResourceCountShape,
        worktrees: BridgeResourceCountShape,
        agents: BridgeResourceCountShape,
    },
    Redacted,
    InventoryLimited,
}
#[allow(dead_code)]
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgeCapacityReasonShape {
    PartitionLimitReached,
    SandboxReservationLimitReached,
    RetiredSandboxReservationsRetained,
    SharedParentPolicyApplies,
    ResourceAccessRequired,
    InventoryLimit,
}
