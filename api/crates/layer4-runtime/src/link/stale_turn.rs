use super::{
    protocol_failure, runtime_failure, HeaderMap, IntoResponse, Json, Path, Response, State,
    StatusCode, Supervisor,
};
use crate::bridge::Bridge;
use crate::manager_ownership::OwnerSession;
use layer3_adapter::stale_turn::{StaleTurnHold, StaleTurnTarget};
use std::sync::Arc;

async fn open(
    supervisor: &Supervisor,
    agent_id: &str,
    headers: &HeaderMap,
    target: &StaleTurnTarget,
) -> Result<(Arc<Bridge>, OwnerSession), Response> {
    let owner = super::conversation::ownership_header(headers)
        .map_err(runtime_failure)?
        .filter(|owner| owner.agent_id == agent_id && owner.epoch == target.writer_epoch)
        .ok_or_else(|| hold(StaleTurnHold::Unconfirmed))?;
    if target.slot.0 != agent_id {
        return Err(hold(StaleTurnHold::Unconfirmed));
    }
    let authority = supervisor
        .manager_ownership
        .session(agent_id, Some(owner))
        .await
        .map_err(runtime_failure)?;
    let bridge = supervisor.stale_turn_bridge(target).await.map_err(hold)?;
    Ok((bridge, authority))
}

pub(super) async fn stop(
    Path(agent_id): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    headers: HeaderMap,
    Json(target): Json<StaleTurnTarget>,
) -> Response {
    let (bridge, authority) = match open(&supervisor, &agent_id, &headers, &target).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(_permit) = authority.permit().await else {
        return hold(StaleTurnHold::Unconfirmed);
    };
    match bridge.request_stale_turn_stop(&target, authority).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(error) => hold(error),
    }
}

pub(super) async fn observe(
    Path(agent_id): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    headers: HeaderMap,
    Json(target): Json<StaleTurnTarget>,
) -> Response {
    let (bridge, authority) = match open(&supervisor, &agent_id, &headers, &target).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(_permit) = authority.permit().await else {
        return hold(StaleTurnHold::Unconfirmed);
    };
    match bridge.observe_stale_turn_stop(&target).await {
        Ok(seal) => Json(seal).into_response(),
        Err(error) => hold(error),
    }
}

fn hold(error: StaleTurnHold) -> Response {
    let (status, code) = match error {
        StaleTurnHold::Unsupported => (StatusCode::NOT_IMPLEMENTED, "stale_turn_unsupported"),
        StaleTurnHold::Unconfirmed => (StatusCode::CONFLICT, "stale_turn_unconfirmed"),
        StaleTurnHold::CanonicalResult => (StatusCode::CONFLICT, "stale_turn_canonical_result"),
    };
    protocol_failure(status, code, error.to_string(), false)
}
