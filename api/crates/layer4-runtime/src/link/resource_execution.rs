use std::sync::{Arc, OnceLock};

use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer3_adapter::{
    ingress_budget::{IngressBudget, IngressReservation},
    resource_execution_protocol::*,
    ResourceInput,
};
use serde::Deserialize;
use tokio::sync::Semaphore;

use crate::supervisor::Supervisor;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    protocol: u32,
    ticket: ExecutionTicketV1,
    input: Box<serde_json::value::RawValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilitiesRequest {
    protocol: u32,
}

pub(super) fn router() -> axum::Router<Arc<Supervisor>> {
    axum::Router::new().route(
        "/runtime/resource-execution/v1/{agent_id}/{action}",
        axum::routing::post(dispatch),
    )
}

async fn dispatch(
    Path((agent, action)): Path<(String, String)>,
    State(supervisor): State<Arc<Supervisor>>,
    request: Request,
) -> Response {
    if request
        .extensions()
        .get::<crate::control_auth::AuthenticatedControl>()
        .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !matches!(
        action.as_str(),
        "capabilities" | "challenge" | "start" | "continue" | "barrier" | "cancel" | "stop"
    ) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let owner = match super::conversation::ownership_header(request.headers()) {
        Ok(Some(owner)) if owner.agent_id == agent => owner,
        _ => return failure(ProtocolError::AudienceMismatch),
    };
    let authority = match supervisor
        .manager_ownership
        .session(&agent, Some(owner.clone()))
        .await
    {
        Ok(authority) => authority,
        Err(_) => return failure(ProtocolError::AudienceMismatch),
    };
    if authority.permit().await.is_err() {
        return failure(ProtocolError::AudienceMismatch);
    }
    let receiver = supervisor.resource_execution.clone();
    let lane = if matches!(action.as_str(), "start" | "challenge") {
        &receiver.admissions
    } else {
        &receiver.controls
    };
    let Ok(_control) = lane.clone().try_acquire_owned() else {
        return failure(ProtocolError::Capacity);
    };
    static CUSTODY: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let Some(_custody) = IngressReservation::try_acquire(
        CUSTODY.get_or_init(|| Arc::new(Semaphore::new(8 * 1024 * 1024))),
        Some(&IngressBudget::process()),
        4 * MAX_MESSAGE_BYTES,
    ) else {
        return failure(ProtocolError::Capacity);
    };
    let body = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        to_bytes(request.into_body(), MAX_MESSAGE_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        _ => return failure(ProtocolError::Capacity),
    };
    if authority.permit().await.is_err() {
        return failure(ProtocolError::AudienceMismatch);
    }
    // The admitted owner keeps its body and both reservations after a lost
    // HTTP waiter; cancellation cannot create unaccounted queued controls.
    let operation = tokio::spawn(async move {
        let _control = _control;
        let _custody = _custody;
        let result = execute(&receiver, &authority, owner, &action, &body).await;
        if authority.permit().await.is_err() {
            return failure(ProtocolError::Unknown);
        }
        match result {
            Ok(reply) => Json(reply).into_response(),
            Err(error) => failure(error),
        }
    });
    operation
        .await
        .unwrap_or_else(|_| failure(ProtocolError::Unknown))
}

async fn execute(
    receiver: &Arc<crate::resource_execution::Receiver>,
    authority: &crate::manager_ownership::OwnerSession,
    owner: layer3_adapter::owner_protocol::TransportOwner,
    action: &str,
    body: &[u8],
) -> ProtocolResult<serde_json::Value> {
    match action {
        "capabilities" => {
            let request: CapabilitiesRequest = decode_message(body)?;
            version(request.protocol)?;
            let capability = receiver.capabilities().map(|capability| serde_json::json!({
                    "protocol": capability.protocol, "targeted_native_stop": capability.targeted_native_stop,
                    "mediated_effects": capability.mediated_effects, "mixed_principal_control": capability.mixed_principal_control,
                }));
            Ok(
                serde_json::json!({"protocol": PROTOCOL_VERSION, "receiver": receiver.identity(), "capabilities": capability}),
            )
        }
        "challenge" => encode(
            receiver
                .challenge(owner.clone(), decode_message(body)?)
                .await?,
        ),
        "start" => {
            let request: StartRequest = decode_message(body)?;
            version(request.protocol)?;
            let dispatch =
                ResourceInput::decode(request.input.get().as_bytes())?.bind(request.ticket)?;
            encode(receiver.start(authority.clone(), dispatch).await?)
        }
        "continue" => {
            receiver
                .continue_execution(authority.clone(), decode_message(body)?)
                .await?;
            Ok(serde_json::json!({"protocol": PROTOCOL_VERSION}))
        }
        "barrier" => encode(
            receiver
                .barrier(owner.clone(), decode_message(body)?)
                .await?,
        ),
        "cancel" => encode(
            receiver
                .stop(owner.clone(), decode_message(body)?, true)
                .await?
                .ok_or(ProtocolError::Unknown)?,
        ),
        "stop" => encode(receiver.stop(owner, decode_message(body)?, false).await?),
        _ => Err(ProtocolError::Unsupported),
    }
}

fn encode(value: impl serde::Serialize) -> ProtocolResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|_| ProtocolError::Unknown)
}

fn failure(error: ProtocolError) -> Response {
    let (status, code) = match error {
        ProtocolError::Unsupported => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
        ProtocolError::InvalidIdentity => (StatusCode::BAD_REQUEST, "invalid_identity"),
        ProtocolError::AudienceMismatch => (StatusCode::FORBIDDEN, "audience_mismatch"),
        ProtocolError::EvidenceMismatch => (StatusCode::CONFLICT, "evidence_mismatch"),
        ProtocolError::Expired => (StatusCode::CONFLICT, "expired"),
        ProtocolError::Capacity => (StatusCode::TOO_MANY_REQUESTS, "capacity"),
        ProtocolError::Unknown => (StatusCode::CONFLICT, "unknown"),
        ProtocolError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    (
        status,
        Json(serde_json::json!({"protocol": PROTOCOL_VERSION, "error": code})),
    )
        .into_response()
}

#[cfg(test)]
#[path = "resource_execution_tests.rs"]
mod tests;
