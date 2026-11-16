//! Master-key loopback operations; no downstream transport authority.
use super::{
    unresolved::{Operator, Request},
    Error,
};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use cmaeg_core::seal::{open_connection, seal_connection, MasterKeyRing};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, sync::Arc};

pub(crate) const PATH: &str = "/internal/replicas/unresolved";
const PURPOSE: &str = "cmaeg_replica_operator_v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Command {
    pub action: String,
    pub request: Option<Request>,
    pub audit: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    purpose: String,
    deployment: String,
    port: u16,
    path: String,
    method: String,
    body: String,
    issued_at: i64,
    expires_at: i64,
}
#[derive(Clone)]
struct Control {
    operator: Operator,
    keys: Arc<MasterKeyRing>,
    deployment: String,
    port: u16,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn ticket(
    keys: &MasterKeyRing,
    origin: &str,
    port: u16,
    body: &[u8],
    now: i64,
) -> Result<String, Error> {
    let ticket = Ticket {
        purpose: PURPOSE.into(),
        deployment: digest(origin.as_bytes()),
        port,
        path: PATH.into(),
        method: "POST".into(),
        body: digest(body),
        issued_at: now,
        expires_at: now + 25,
    };
    let bytes = seal_connection(
        keys,
        PURPOSE,
        &serde_json::to_vec(&ticket).map_err(|_| Error::Corrupt)?,
    )
    .map_err(|_| Error::Corrupt)?;
    String::from_utf8(bytes).map_err(|_| Error::Corrupt)
}
pub(crate) fn router(
    operator: Operator,
    keys: Arc<MasterKeyRing>,
    origin: &str,
    port: u16,
) -> Router {
    Router::new().route(PATH, post(handle)).with_state(Control {
        operator,
        keys,
        deployment: digest(origin.as_bytes()),
        port,
    })
}
async fn handle(
    State(control): State<Control>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let authorized = || -> Option<u8> {
        if !peer.ip().is_loopback() || body.len() > 8192 {
            return None;
        }
        let raw = headers
            .get("authorization")?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        let opened = open_connection(&control.keys, PURPOSE, raw.as_bytes()).ok()?;
        let t: Ticket = serde_json::from_slice(&opened.payload).ok()?;
        (t.purpose == PURPOSE
            && t.deployment == control.deployment
            && t.port == control.port
            && t.path == PATH
            && t.method == "POST"
            && t.body == digest(&body)
            && t.issued_at <= now
            && t.expires_at == t.issued_at + 25
            && t.expires_at > now)
            .then_some(opened.kid)
    };
    let Some(authority_key_id) = authorized() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Ok(command) = serde_json::from_slice::<Command>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mutation = command.action == "apply";
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        match (command.action.as_str(), command.request, command.audit) {
            ("list", None, None) => {
                serde_json::to_value(control.operator.inspect().await?).map_err(|_| Error::Corrupt)
            }
            ("dry_run", Some(req), None) => {
                serde_json::to_value(control.operator.preview(&req).await?)
                    .map_err(|_| Error::Corrupt)
            }
            ("apply", Some(req), None) => {
                serde_json::to_value(control.operator.apply(&req, authority_key_id).await?)
                    .map_err(|_| Error::Corrupt)
            }
            ("audit", None, Some(id)) => {
                serde_json::to_value(control.operator.audit(&id).await?).map_err(|_| Error::Corrupt)
            }
            _ => Err(Error::Corrupt),
        }
    })
    .await
    .unwrap_or(Err(Error::Unavailable));
    match result {
        Ok(value) => ([("cache-control", "no-store")], Json(value)).into_response(),
        Err(error) => (StatusCode::CONFLICT, Json(serde_json::json!({"code":match error { Error::Lost => "capability_or_evidence_unavailable", Error::Busy => "binding_or_live_authority_conflict", Error::Expired => "evidence_expired", Error::Capacity => "inspection_capacity", Error::Corrupt => "invalid_operator_evidence", Error::Unavailable if mutation => "operator_outcome_uncertain", Error::Unavailable => "store_unavailable" }}))).into_response(),
    }
}
