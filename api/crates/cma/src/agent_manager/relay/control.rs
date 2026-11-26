//! Local preStop drain uses the existing deployment signing authority.
use super::*;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::SocketAddr;

pub(crate) const DRAIN: &str = "/internal/agent-manager/v1/drain";

#[derive(Clone)]
struct Control {
    manager: Arc<crate::agent_manager::AgentManager>,
    relay: Arc<Relay>,
}

pub(super) fn router(
    router: axum::Router,
    manager: Arc<crate::agent_manager::AgentManager>,
    relay: Arc<Relay>,
) -> axum::Router {
    router.layer(axum::middleware::from_fn_with_state(
        Control { manager, relay },
        admission,
    ))
}

async fn admission(State(state): State<Control>, request: Request, next: Next) -> Response {
    if request.uri().path() == "/ready" && !state.manager.is_ready() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"status":"not-ready", "reason":"membership"})),
        )
            .into_response();
    }
    if request.uri().path() != DRAIN {
        return next.run(request).await;
    }
    if request.method() != Method::POST
        || !request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|peer| peer.0.ip().is_loopback())
        || request.headers().get_all(PROOF).iter().count() != 1
    {
        tracing::warn!(
            status = 403_u16,
            reason = "invalid_request",
            "local Agent Manager drain refused"
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    let proof = request
        .headers()
        .get(PROOF)
        .and_then(|value| value.to_str().ok())
        .and_then(|proof| {
            state
                .relay
                .authentication
                .verify(proof, &Method::POST, request.uri(), request.headers(), &[])
                .ok()
        });
    if proof.is_none_or(|proof| proof.owner != drain_token() || proof.sender != "prestop") {
        tracing::warn!(
            status = 403_u16,
            reason = "invalid_proof",
            "local Agent Manager drain refused"
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    let admission = request
        .extensions()
        .get::<crate::startup::Admission>()
        .cloned();
    let result = match admission {
        Some(admission) => admission.drain().await,
        None => state.manager.drain().await,
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => {
            tracing::warn!(
                status = 503_u16,
                reason = "incomplete_drain",
                "local Agent Manager drain refused"
            );
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

fn drain_token() -> OwnershipToken {
    OwnershipToken {
        agent_id: AgentId("manager-drain".into()),
        instance_id: "prestop".into(),
        epoch: 0,
        protocol: layer2_core::agent_ownership::PROTOCOL_VERSION,
    }
}

pub(crate) async fn prestop(config: &crate::config::Config) -> Result<(), String> {
    if config.agent_manager_replicas.is_none() {
        return Ok(());
    }
    drain_local(
        config.bind.port(),
        config
            .auth_encryption_key
            .agent_manager_signing_key(&config.deployment_id),
    )
    .await
}

pub(super) fn signed_drain_headers(key: ring::hmac::Key) -> Result<HeaderMap, String> {
    let uri: Uri = DRAIN.parse().unwrap();
    authentication::Authentication::new(key)
        .signed_headers(
            "prestop",
            &drain_token(),
            &Method::POST,
            &uri,
            HeaderMap::new(),
            &[],
        )
        .map_err(|_| "cannot sign drain request".into())
}

pub(super) async fn drain_local(port: u16, key: ring::hmac::Key) -> Result<(), String> {
    let headers = signed_drain_headers(key)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "cannot construct drain client")?;
    let response = client
        .post(format!("http://127.0.0.1:{port}{DRAIN}"))
        .headers(headers)
        .send()
        .await
        .map_err(|_| "local Agent Manager drain failed")?;
    if response.status() == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(format!(
            "local Agent Manager rejected drain (HTTP {})",
            response.status().as_u16()
        ))
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
