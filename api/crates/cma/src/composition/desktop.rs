//! Owner-authenticated desktop projection for Machine Included Agents.
//!
//! The browser receives a small noVNC host page from CMA.  Its WebSocket is
//! authorized against the same Agent lineage and then relayed through the
//! adapter, which keeps the Kubernetes port-forward and runtime token private.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{ws::WebSocket, Path, State, WebSocketUpgrade};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt as _, StreamExt as _};
use layer1_public::AuthenticatedCaller;
use layer2_core::{Capability, RuntimeCapability};
use layer3_adapter::{DesktopFrame, RuntimeHandle, SandboxAdapter};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Clone)]
pub(crate) struct DesktopState {
    reads: Arc<layer2_core::sandbox::AgentReadService>,
    adapter: Arc<dyn SandboxAdapter>,
    leases: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
}

impl DesktopState {
    pub(crate) fn new(
        reads: Arc<layer2_core::sandbox::AgentReadService>,
        adapter: Arc<dyn SandboxAdapter>,
    ) -> Self {
        Self {
            reads,
            adapter,
            leases: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    async fn lease(&self, agent_id: &str) -> OwnedMutexGuard<()> {
        let lease = {
            let mut leases = self.leases.lock().await;
            leases
                .entry(agent_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        lease.lock_owned().await
    }
}

pub(crate) fn router(
    reads: Arc<layer2_core::sandbox::AgentReadService>,
    adapter: Arc<dyn SandboxAdapter>,
) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/desktop", get(page))
        .route("/api/v1/agents/{id}/desktop/ws", get(websocket))
        .with_state(DesktopState::new(reads, adapter))
}

async fn page(
    State(state): State<DesktopState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    if authorize(&state, &caller, &id).await.is_none() {
        return problem(StatusCode::NOT_FOUND, "desktop_unavailable");
    }
    let mut response = Html(desktop_html()).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src https://cdn.jsdelivr.net; style-src 'unsafe-inline'; connect-src 'self' ws: wss:;",
        ),
    );
    response
}

async fn websocket(
    ws: WebSocketUpgrade,
    State(state): State<DesktopState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(runtime_id) = authorize(&state, &caller, &id).await else {
        return problem(StatusCode::NOT_FOUND, "desktop_unavailable");
    };
    let lease = state.lease(&id).await;
    ws.on_upgrade(move |socket| relay(state, runtime_id, socket, lease))
        .into_response()
}

async fn authorize(state: &DesktopState, caller: &layer2_core::Caller, id: &str) -> Option<String> {
    if !caller.authorization.allows(Capability::UseOwnSandboxes) {
        return None;
    }
    let lineage = state
        .reads
        .lineage(caller, &layer2_core::AgentId(id.to_owned()))
        .await
        .ok()?;
    if !lineage
        .agent
        .capabilities
        .supports(RuntimeCapability::Desktop)
        || !lineage
            .sandbox
            .profile
            .content
            .environment_variables
            .iter()
            .any(|variable| variable.name == "CMA_MACHINE_INCLUDED" && variable.value == "1")
    {
        return None;
    }
    lineage
        .sandbox
        .runtime
        .filter(|runtime| !runtime.runtime_id.trim().is_empty())
        .map(|runtime| runtime.runtime_id)
}

async fn relay(
    state: DesktopState,
    runtime_id: String,
    socket: WebSocket,
    _lease: OwnedMutexGuard<()>,
) {
    let Ok(mut upstream) = state
        .adapter
        .open_desktop(&RuntimeHandle::new(runtime_id))
        .await
    else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    loop {
        tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(message)) => {
                    let Some(frame) = browser_frame(message) else { return };
                    if upstream.send(frame).await.is_err() { return; }
                }
                Some(Err(_)) | None => return,
            },
            outgoing = upstream.recv() => match outgoing {
                Ok(Some(frame)) => {
                    let Some(message) = runtime_message(frame) else { return; };
                    if sink.send(message).await.is_err() { return; }
                }
                Ok(None) | Err(_) => return,
            },
        }
    }
}

fn browser_frame(message: axum::extract::ws::Message) -> Option<DesktopFrame> {
    match message {
        axum::extract::ws::Message::Binary(bytes) => Some(DesktopFrame::Binary(bytes.to_vec())),
        axum::extract::ws::Message::Text(text) => Some(DesktopFrame::Text(text.to_string())),
        axum::extract::ws::Message::Ping(bytes) => Some(DesktopFrame::Ping(bytes.to_vec())),
        axum::extract::ws::Message::Pong(bytes) => Some(DesktopFrame::Pong(bytes.to_vec())),
        axum::extract::ws::Message::Close(_) => Some(DesktopFrame::Close),
    }
}

fn runtime_message(frame: DesktopFrame) -> Option<axum::extract::ws::Message> {
    match frame {
        DesktopFrame::Binary(bytes) => Some(axum::extract::ws::Message::Binary(bytes.into())),
        DesktopFrame::Text(text) => Some(axum::extract::ws::Message::Text(text.into())),
        DesktopFrame::Ping(bytes) => Some(axum::extract::ws::Message::Ping(bytes.into())),
        DesktopFrame::Pong(bytes) => Some(axum::extract::ws::Message::Pong(bytes.into())),
        DesktopFrame::Close => Some(axum::extract::ws::Message::Close(None)),
    }
}

fn problem(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "type": format!("https://spec.chrono-ai.fun/cma/1.0/problems/{code}"),
            "title": code,
            "status": status.as_u16(),
            "code": code,
        })),
    )
        .into_response()
}

fn desktop_html() -> &'static str {
    r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Machine desktop</title>
<style>html,body,#screen{margin:0;width:100%;height:100%;overflow:hidden;background:#111}#screen{display:block}#status{position:fixed;left:12px;bottom:12px;padding:6px 9px;border-radius:6px;background:#222c;color:#fff;font:12px system-ui}</style></head>
<body><div id="screen"></div><div id="status">Connecting…</div>
<script type="module">
import RFB from 'https://cdn.jsdelivr.net/npm/@novnc/novnc@1.7.0/core/rfb.js';
const ws = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${location.pathname}/ws`;
const rfb = new RFB(document.getElementById('screen'), ws);
rfb.scaleViewport = true; rfb.resizeSession = true;
rfb.addEventListener('connect', () => document.getElementById('status').textContent = 'Connected');
rfb.addEventListener('disconnect', () => document.getElementById('status').textContent = 'Disconnected');
</script></body></html>"#
}
