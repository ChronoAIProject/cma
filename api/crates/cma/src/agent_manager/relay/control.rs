//! Local preStop drain uses the existing deployment signing authority.
use super::*;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::SocketAddr;

const DRAIN: &str = "/internal/agent-manager/v1/drain";

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
        return StatusCode::FORBIDDEN.into_response();
    }
    state.manager.drain().await;
    StatusCode::NO_CONTENT.into_response()
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
    let authentication = authentication::Authentication::new(
        config
            .auth_encryption_key
            .agent_manager_signing_key(&config.deployment_id),
    );
    let uri: Uri = DRAIN.parse().unwrap();
    let proof = authentication
        .sign(
            "prestop",
            &drain_token(),
            &Method::POST,
            &uri,
            &HeaderMap::new(),
            &[],
        )
        .map_err(|_| "cannot sign drain request")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "cannot construct drain client")?;
    let response = client
        .post(format!("http://127.0.0.1:{}{DRAIN}", config.bind.port()))
        .header(PROOF, proof)
        .send()
        .await
        .map_err(|_| "local Agent Manager drain failed")?;
    if response.status() == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err("local Agent Manager rejected drain".into())
    }
}
