//! Public desktop discovery and owner-authenticated binary stream.
use std::future::Future;
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
    Extension, Json, Router,
};
use futures_util::{Sink, SinkExt as _, Stream, StreamExt as _};
use layer2_core::{
    sandbox::{DesktopConnection, DesktopError, DesktopFrame, DesktopService},
    AgentId, Caller,
};
use tokio::time::Instant;

use crate::desktop_telemetry::{CloseReason, Direction, RelayObservation};
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
        (status = 404, body = super::ProblemDocument),
        (status = 409, body = super::ProblemDocument),
        (status = 503, body = super::ProblemDocument)))]
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

/// Apply the desktop browser origin policy before any upgrade.
pub fn permitted_origin(headers: &HeaderMap, origins: &[String]) -> bool {
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
    authorization: Option<Extension<Arc<dyn layer2_core::sandbox::DesktopAuthorization>>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !permitted_origin(&headers, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let id = AgentId(id);
    let opened = match authorization {
        Some(Extension(authority)) => state.service.open_authorized(&caller, &id, authority).await,
        None => state.service.open(&caller, &id).await,
    };
    let connection = match opened {
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
    let mut observation = RelayObservation::start();
    let reason = relay_frames(
        &mut sink,
        &mut source,
        &mut DesktopRelay {
            service: &service,
            caller: &caller,
            id: &id,
            connection: &mut connection,
        },
        &mut observation,
    )
    .await;
    observation.finish(reason);
    service.close_connection(&mut connection).await;
    drop(connection);
    let _ = tokio::time::timeout(Duration::from_secs(2), sink.send(Message::Close(None))).await;
}

/// The frame pump uses the admitted stream's authorization and never manages
/// renewal or the underlying transport itself.
trait RelayRuntime {
    fn still_authorized(&mut self) -> impl Future<Output = bool> + Send;
    fn send(
        &mut self,
        frame: DesktopFrame,
    ) -> impl Future<Output = Result<(), DesktopError>> + Send;
    fn recv(&mut self) -> impl Future<Output = Result<Option<DesktopFrame>, DesktopError>> + Send;
    fn while_authorized<T: Send>(
        &mut self,
        operation: impl Future<Output = T> + Send,
    ) -> impl Future<Output = Result<T, DesktopError>> + Send;
}

struct DesktopRelay<'a> {
    service: &'a DesktopService,
    caller: &'a Caller,
    id: &'a AgentId,
    connection: &'a mut DesktopConnection,
}

impl RelayRuntime for DesktopRelay<'_> {
    async fn still_authorized(&mut self) -> bool {
        self.service
            .still_authorized(self.caller, self.id, self.connection)
            .await
    }

    async fn send(&mut self, frame: DesktopFrame) -> Result<(), DesktopError> {
        self.connection.send(frame).await
    }

    async fn recv(&mut self) -> Result<Option<DesktopFrame>, DesktopError> {
        self.connection.recv().await
    }

    async fn while_authorized<T: Send>(
        &mut self,
        operation: impl Future<Output = T> + Send,
    ) -> Result<T, DesktopError> {
        self.connection.while_authorized(operation).await
    }
}

async fn relay_frames<S, R, E>(
    sink: &mut S,
    source: &mut R,
    runtime: &mut impl RelayRuntime,
    observation: &mut RelayObservation<'_>,
) -> CloseReason
where
    S: Sink<Message> + Unpin + Send,
    S::Error: Send,
    R: Stream<Item = Result<Message, E>> + Unpin,
{
    let mut authorization = tokio::time::interval(Duration::from_secs(30));
    let lifetime = tokio::time::sleep(Duration::from_secs(30 * 60));
    tokio::pin!(lifetime);
    loop {
        tokio::select! {
            _ = &mut lifetime => return CloseReason::Lifetime,
            _ = authorization.tick() => {
                if !observation.check_authorization(runtime.still_authorized()).await {
                    return CloseReason::AuthorizationRefused;
                }
            },
            incoming = source.next() => {
                let frame = match incoming {
                    Some(Ok(Message::Binary(bytes))) => DesktopFrame::Binary(bytes.to_vec()),
                    Some(Ok(Message::Text(text))) => DesktopFrame::Text(text.to_string()),
                    Some(Ok(Message::Ping(bytes))) => DesktopFrame::Ping(bytes.to_vec()),
                    Some(Ok(Message::Pong(bytes))) => DesktopFrame::Pong(bytes.to_vec()),
                    Some(Ok(Message::Close(_))) | None => return CloseReason::ClientClosed,
                    Some(Err(_)) => return CloseReason::ClientError,
                };
                let bytes = frame_bytes(&frame);
                let started = Instant::now();
                let result = tokio::time::timeout(Duration::from_secs(10), runtime.send(frame)).await;
                observation.send_wait(Direction::Uplink, started.elapsed());
                match result {
                    Ok(Ok(())) => observation.forwarded(Direction::Uplink, bytes),
                    Ok(Err(DesktopError::Unavailable | DesktopError::AuthorityUnavailable)) =>
                        return CloseReason::AuthorizationRefused,
                    Ok(Err(_)) => return CloseReason::RuntimeError,
                    Err(_) => return CloseReason::RuntimeSendTimeout,
                }
            },
            outgoing = runtime.recv() => {
                let frame = match outgoing {
                    Ok(Some(frame)) => frame,
                    Ok(None) => return CloseReason::RuntimeClosed,
                    Err(DesktopError::Unavailable | DesktopError::AuthorityUnavailable) =>
                        return CloseReason::AuthorizationRefused,
                    Err(_) => return CloseReason::RuntimeError,
                };
                let bytes = frame_bytes(&frame);
                let message = match frame {
                    DesktopFrame::Binary(bytes) => Message::Binary(bytes.into()),
                    DesktopFrame::Text(text) => Message::Text(text.into()),
                    DesktopFrame::Ping(bytes) => Message::Ping(bytes.into()),
                    DesktopFrame::Pong(bytes) => Message::Pong(bytes.into()),
                    DesktopFrame::Close => return CloseReason::RuntimeClosed,
                };
                let started = Instant::now();
                let result = runtime.while_authorized(
                    tokio::time::timeout(Duration::from_secs(10), sink.send(message))
                ).await;
                observation.send_wait(Direction::Downlink, started.elapsed());
                match result {
                    Ok(Ok(Ok(()))) => observation.forwarded(Direction::Downlink, bytes),
                    Ok(Ok(Err(_))) => return CloseReason::ClientError,
                    Ok(Err(_)) => return CloseReason::ClientSendTimeout,
                    Err(_) => return CloseReason::AuthorizationRefused,
                };
            },
        }
    }
}

#[cfg(test)]
#[path = "desktop/relay_tests.rs"]
mod relay_tests;

fn frame_bytes(frame: &DesktopFrame) -> usize {
    match frame {
        DesktopFrame::Binary(bytes) | DesktopFrame::Ping(bytes) | DesktopFrame::Pong(bytes) => {
            bytes.len()
        }
        DesktopFrame::Text(text) => text.len(),
        DesktopFrame::Close => 0,
    }
}

fn problem(error: DesktopError) -> Response {
    let (status, code) = match error {
        DesktopError::RuntimeCapacity(capacity) => {
            return super::problems::runtime_capacity(capacity)
        }
        DesktopError::Unavailable => (StatusCode::NOT_FOUND, "desktop_unavailable"),
        DesktopError::Busy => (StatusCode::CONFLICT, "desktop_busy"),
        DesktopError::RuntimeUnavailable | DesktopError::RuntimeFailed => (
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop_runtime_unavailable",
        ),
        DesktopError::AuthorityUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop_authority_unavailable",
        ),
    };
    let body = super::problem_document(status, code, &error.to_string(), code);
    let mut response = super::problem_response(status, body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store, private".parse().unwrap());
    if matches!(
        error,
        DesktopError::Busy | DesktopError::RuntimeUnavailable | DesktopError::AuthorityUnavailable
    ) {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, "1".parse().unwrap());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_desktop_failure_preserves_the_public_problem_and_removes_retry_after() {
        for (error, retryable) in [
            (DesktopError::RuntimeUnavailable, true),
            (DesktopError::RuntimeFailed, false),
        ] {
            let detail = error.to_string();
            let response = problem(error);
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .map(|value| value.to_str().unwrap()),
                retryable.then_some("1")
            );
            let body = axum::body::to_bytes(response.into_body(), 2048)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["code"], "desktop_runtime_unavailable");
            assert_eq!(body["detail"], detail);
        }
    }

    #[test]
    fn busy_discovery_exposes_a_private_retry_deadline() {
        let response = problem(DesktopError::Busy);
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, private"
        );
        assert!(!problem(DesktopError::Unavailable)
            .headers()
            .contains_key(header::RETRY_AFTER));
    }
    #[test]
    fn shared_authority_failure_is_retryable_service_unavailability() {
        assert_eq!(
            problem(DesktopError::AuthorityUnavailable).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(problem(DesktopError::Busy).status(), StatusCode::CONFLICT);
    }

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
