use crate::supervisor::Supervisor;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use layer3_adapter::owner_protocol::{OwnerConfirmation, TransportOwner};
use layer3_adapter::wire::{RequestFrame, ResponseFrame};
use std::sync::Arc;

fn require_agent(agent: &str, owner: &TransportOwner) -> Result<(), crate::failure::RuntimeError> {
    if owner.agent_id != agent {
        return Err(crate::failure::RuntimeError::rejected(
            "manager_ownership_identity",
            "ownership token names another Agent",
        ));
    }
    owner.validate().map_err(|_| {
        crate::failure::RuntimeError::rejected(
            "manager_ownership_invalid",
            "ownership token is invalid",
        )
    })
}

pub(super) async fn dispatch(
    supervisor: &Supervisor,
    request: RequestFrame,
    owner: Option<TransportOwner>,
) -> ResponseFrame {
    let operation = request.operation();
    let target = layer3_adapter::owner_control::agent_target(&request)
        .map(str::to_owned)
        .or_else(|| owner.as_ref().map(|owner| owner.agent_id.clone()));
    let Some(target) = target else {
        return supervisor.dispatch(request).await;
    };
    let session = match supervisor.manager_ownership.session(&target, owner).await {
        Ok(session) => session,
        Err(error) => return error.response(operation),
    };
    let permit = match session.permit().await {
        Ok(permit) => permit,
        Err(error) => return error.response(operation),
    };
    let response = supervisor.dispatch(request).await;
    drop(permit);
    if session.permit().await.is_err() {
        crate::failure::RuntimeError::rejected(
            "manager_ownership_stale",
            "transport ownership expired during control execution",
        )
        .response(operation)
    } else {
        response
    }
}

pub(super) async fn challenge(
    Path(agent): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    Json(owner): Json<TransportOwner>,
) -> Response {
    if let Err(error) = require_agent(&agent, &owner) {
        return super::runtime_failure(error);
    }
    match supervisor.manager_ownership.challenge(owner).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => super::runtime_failure(error),
    }
}

pub(super) async fn confirm(
    Path(agent): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    Json(request): Json<OwnerConfirmation>,
) -> Response {
    if let Err(error) = require_agent(&agent, &request.owner) {
        return super::runtime_failure(error);
    }
    match supervisor.manager_ownership.confirm(request).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => super::runtime_failure(error),
    }
}

pub(super) async fn revoke(
    Path(agent): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    Json(owner): Json<TransportOwner>,
) -> Response {
    if let Err(error) = require_agent(&agent, &owner) {
        return super::runtime_failure(error);
    }
    match supervisor.manager_ownership.revoke(owner).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => super::runtime_failure(error),
    }
}

pub(super) async fn reset_legacy(
    Path(agent): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    Json(owner): Json<TransportOwner>,
) -> Response {
    if let Err(error) = require_agent(&agent, &owner) {
        return super::runtime_failure(error);
    }
    match supervisor.manager_ownership.reset_legacy(owner).await {
        Ok(reply) => Json(reply).into_response(),
        Err(error) => super::runtime_failure(error),
    }
}
