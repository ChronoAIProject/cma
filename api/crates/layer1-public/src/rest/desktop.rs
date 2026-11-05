//! Public desktop discovery and owner-authenticated binary stream.
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt as _, StreamExt as _};
use layer2_core::{
    sandbox::{DesktopConnection, DesktopError, DesktopFrame, DesktopService},
    AgentId, Caller,
};

use crate::AuthenticatedCaller;

#[derive(Clone)]
struct DesktopState {
    service: Arc<DesktopService>,
    origins: Vec<String>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub(super) struct DesktopDescriptor {
    /// Same-origin authenticated WebSocket carrying RFB binary frames.
    websocket_url: String,
}

pub fn desktop_router(service: Arc<DesktopService>, origin: String) -> Router {
    desktop_router_with_origins(service, vec![origin])
}

/// Deployment-owned exact browser origins, including retained domain aliases.
/// Never derive this allowlist from Host, Forwarded or the incoming Origin.
pub fn desktop_router_with_origins(service: Arc<DesktopService>, origins: Vec<String>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/desktop", get(describe))
        .route("/api/v1/agents/{id}/desktop/ws", get(websocket))
        .with_state(DesktopState { service, origins })
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/desktop", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = DesktopDescriptor),
        (status = 404, body = super::ProblemDocument)))]
async fn describe(
    State(state): State<DesktopState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match state.service.available(&caller, &AgentId(id.clone())).await {
        Ok(()) => (
            [(header::CACHE_CONTROL, "no-store, private")],
            Json(DesktopDescriptor {
                websocket_url: format!("/api/v1/agents/{id}/desktop/ws"),
            }),
        )
            .into_response(),
        Err(error) => problem(error),
    }
}

fn permitted_origin(headers: &HeaderMap, origins: &[String]) -> bool {
    match headers.get(header::ORIGIN) {
        Some(value) => {
            headers.get_all(header::ORIGIN).iter().count() == 1
                && origins
                    .iter()
                    .any(|origin| value.to_str().ok() == Some(origin.as_str()))
        }
        None => !headers.contains_key(header::COOKIE),
    }
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/desktop/ws", tag = "agents",
    params(("id" = String, Path), ("Origin" = Option<String>, Header,
        description = "Exact CMA browser origin; required with cookie authentication")),
    responses((status = 101, description = "Authenticated RFB stream over binary WebSocket frames"),
        (status = 400, description = "WebSocket upgrade required"),
        (status = 403, description = "Browser origin refused"),
        (status = 404, body = super::ProblemDocument),
        (status = 409, description = "Another browser owns the Sandbox desktop connection", body = super::ProblemDocument),
        (status = 503, body = super::ProblemDocument)))]
async fn websocket(
    State(state): State<DesktopState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !permitted_origin(&headers, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let id = AgentId(id);
    let connection = match state.service.open(&caller, &id).await {
        Ok(connection) => connection,
        Err(error) => return problem(error),
    };
    ws.max_message_size(8 * 1024 * 1024)
        .max_frame_size(8 * 1024 * 1024)
        .on_upgrade(move |socket| relay(state.service, caller, id, socket, connection))
        .into_response()
}

async fn relay(
    service: Arc<DesktopService>,
    caller: Caller,
    id: AgentId,
    socket: WebSocket,
    mut connection: DesktopConnection,
) {
    let (mut sink, mut source) = socket.split();
    let mut authorization = tokio::time::interval(Duration::from_secs(30));
    let lifetime = tokio::time::sleep(Duration::from_secs(30 * 60));
    tokio::pin!(lifetime);
    loop {
        tokio::select! {
            _ = &mut lifetime => break,
            _ = authorization.tick() => {
                if !service.still_authorized(&caller, &id, &mut connection).await { break; }
            },
            incoming = source.next() => {
                let frame = match incoming {
                    Some(Ok(Message::Binary(bytes))) => DesktopFrame::Binary(bytes.to_vec()),
                    Some(Ok(Message::Text(text))) => DesktopFrame::Text(text.to_string()),
                    Some(Ok(Message::Ping(bytes))) => DesktopFrame::Ping(bytes.to_vec()),
                    Some(Ok(Message::Pong(bytes))) => DesktopFrame::Pong(bytes.to_vec()),
                    _ => break,
                };
                if !matches!(tokio::time::timeout(Duration::from_secs(10), connection.send(frame)).await, Ok(Ok(()))) { break; }
            },
            outgoing = connection.recv() => {
                let message = match outgoing {
                    Ok(Some(DesktopFrame::Binary(bytes))) => Message::Binary(bytes.into()),
                    Ok(Some(DesktopFrame::Text(text))) => Message::Text(text.into()),
                    Ok(Some(DesktopFrame::Ping(bytes))) => Message::Ping(bytes.into()),
                    Ok(Some(DesktopFrame::Pong(bytes))) => Message::Pong(bytes.into()),
                    _ => break,
                };
                if !matches!(tokio::time::timeout(Duration::from_secs(10), sink.send(message)).await, Ok(Ok(()))) { break; }
            },
        }
    }
    let (sandbox_id, session_id) = connection.session_ids();
    service.close(sandbox_id, session_id).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), sink.send(Message::Close(None))).await;
}

fn problem(error: DesktopError) -> Response {
    let (status, code) = match error {
        DesktopError::Unavailable => (StatusCode::NOT_FOUND, "desktop_unavailable"),
        DesktopError::Busy => (StatusCode::CONFLICT, "desktop_busy"),
        DesktopError::RuntimeUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop_runtime_unavailable",
        ),
    };
    let body = super::problem_document(status, code, &error.to_string(), code);
    super::problem_response(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_cookie_stream_requires_one_exact_origin() {
        let origins = vec![
            "https://cma.example".into(),
            "https://legacy.example".into(),
        ];
        let mut headers = HeaderMap::new();
        assert!(permitted_origin(&headers, &origins));
        headers.insert(header::COOKIE, "session=fixture".parse().unwrap());
        assert!(!permitted_origin(&headers, &origins));
        headers.insert(header::ORIGIN, "https://foreign.example".parse().unwrap());
        assert!(!permitted_origin(&headers, &origins));
        headers.insert(header::ORIGIN, "null".parse().unwrap());
        assert!(!permitted_origin(&headers, &origins));
        headers.insert(header::ORIGIN, "https://cma.example".parse().unwrap());
        assert!(permitted_origin(&headers, &origins));
        headers.insert(header::ORIGIN, "https://legacy.example".parse().unwrap());
        assert!(permitted_origin(&headers, &origins));
        headers.insert(
            header::ORIGIN,
            "https://cma.example.evil.test".parse().unwrap(),
        );
        assert!(!permitted_origin(&headers, &origins));
        headers.insert(header::ORIGIN, "https://cma.example".parse().unwrap());
        headers.append(header::ORIGIN, "https://legacy.example".parse().unwrap());
        assert!(!permitted_origin(&headers, &origins));
    }
}
