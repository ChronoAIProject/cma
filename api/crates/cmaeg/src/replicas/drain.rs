//! Authenticated loopback drain. Repeating it can never restore readiness.
use crate::turns::TurnTracker;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use cmaeg_core::seal::{open_connection, seal_connection, MasterKeyRing};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::watch;

pub const PATH: &str = "/internal/replicas/drain";
const PURPOSE: &str = "cmaeg_replica_drain_v1";
const TICKET_SECONDS: i64 = 25;

#[derive(Clone)]
pub(crate) struct Control {
    keys: Arc<MasterKeyRing>,
    tracker: Arc<TurnTracker>,
    requested: watch::Sender<bool>,
    port: u16,
    deployment: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    purpose: String,
    method: String,
    path: String,
    deployment: String,
    port: u16,
    issued_at: i64,
    expires_at: i64,
}

impl Control {
    pub fn new(
        keys: Arc<MasterKeyRing>,
        tracker: Arc<TurnTracker>,
        port: u16,
        origin: &str,
    ) -> Self {
        let (requested, _) = watch::channel(false);
        Self {
            keys,
            tracker,
            requested,
            port,
            deployment: deployment(origin),
        }
    }

    pub fn router(&self, enabled: bool) -> Router {
        if enabled {
            Router::new()
                .route(PATH, post(request))
                .with_state(self.clone())
        } else {
            Router::new()
        }
    }

    pub fn start(&self) {
        self.tracker.start_draining();
        if !self.requested.send_replace(true) {
            tracing::info!("Event Gateway replica drain requested");
        }
    }

    pub fn is_draining(&self) -> bool {
        self.tracker.is_draining()
    }

    pub async fn requested(&self) {
        let mut receiver = self.requested.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }

    fn authorize(&self, peer: SocketAddr, ticket: &str, now: i64) -> bool {
        if !peer.ip().is_loopback() {
            return false;
        }
        let Ok(opened) = open_connection(&self.keys, PURPOSE, ticket.as_bytes()) else {
            return false;
        };
        let Ok(ticket) = serde_json::from_slice::<Ticket>(&opened.payload) else {
            return false;
        };
        ticket.purpose == PURPOSE
            && ticket.method == "POST"
            && ticket.path == PATH
            && ticket.deployment == self.deployment
            && ticket.port == self.port
            && ticket.issued_at <= now
            && ticket.issued_at >= now.saturating_sub(TICKET_SECONDS)
            && ticket.expires_at == ticket.issued_at.saturating_add(TICKET_SECONDS)
            && ticket.expires_at > now
    }
}

async fn request(
    State(control): State<Control>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(ticket) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return StatusCode::UNAUTHORIZED;
    };
    if !control.authorize(
        peer,
        ticket,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    ) {
        return StatusCode::UNAUTHORIZED;
    }
    if !body.is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    control.start();
    StatusCode::NO_CONTENT
}

pub(super) fn ticket(
    keys: &MasterKeyRing,
    port: u16,
    origin: &str,
    now: i64,
) -> Result<String, &'static str> {
    let payload = Ticket {
        purpose: PURPOSE.into(),
        method: "POST".into(),
        path: PATH.into(),
        deployment: deployment(origin),
        port,
        issued_at: now,
        expires_at: now.saturating_add(TICKET_SECONDS),
    };
    let payload = serde_json::to_vec(&payload).map_err(|_| "drain authorization unavailable")?;
    let bytes =
        seal_connection(keys, PURPOSE, &payload).map_err(|_| "drain authorization unavailable")?;
    String::from_utf8(bytes).map_err(|_| "drain authorization unavailable")
}

fn deployment(origin: &str) -> String {
    format!("{:x}", Sha256::digest(origin.as_bytes()))
}

#[cfg(test)]
#[path = "drain_tests.rs"]
mod tests;
