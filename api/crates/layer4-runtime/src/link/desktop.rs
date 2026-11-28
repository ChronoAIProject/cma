//! Authenticated relay to the runtime-local noVNC WebSocket.
//!
//! The runtime never exposes port 6080 directly.  The supervisor's control
//! middleware protects this endpoint, and a higher layer may port-forward this
//! same authenticated stream through CMA.

use std::sync::Arc;

use axum::extract::{ws::WebSocket, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};
use layer3_adapter::desktop_ownership::wire::{DESKTOP_STATE_FAILED, DESKTOP_STATE_HEADER};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use super::{protocol_failure, runtime_failure};
use crate::desktop_ownership::Session;
use crate::desktop_process::transport::{connect, Display, DISPLAY_URL};
use crate::desktop_process::DesktopGeneration;
use crate::supervisor::Supervisor;

pub(super) async fn websocket(
    ws: WebSocketUpgrade,
    State(supervisor): State<Arc<Supervisor>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(generation) = supervisor.desktop_generation() else {
        return protocol_failure(
            axum::http::StatusCode::NOT_FOUND,
            "desktop_unavailable",
            "this Sandbox was not provisioned with a desktop machine",
            false,
        );
    };
    let generation = match generation {
        Ok(generation) => generation,
        Err(error) => return unavailable(error.retryable),
    };
    upgrade(
        ws,
        supervisor.desktop_ownership.clone(),
        generation,
        DISPLAY_URL.into(),
        headers,
    )
    .await
}

async fn upgrade(
    ws: WebSocketUpgrade,
    receiver: Arc<crate::desktop_ownership::DesktopReceiver>,
    generation: DesktopGeneration,
    display: String,
    headers: axum::http::HeaderMap,
) -> Response {
    let owner = match ownership(&headers) {
        Ok(owner) => owner,
        Err(error) => return runtime_failure(error),
    };
    let (socket_tx, socket_rx) = tokio::sync::oneshot::channel();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let registered_generation = generation.clone();
    // Register before connecting or exposing the upgrade. The receiver keeps
    // the entire task, including a pending upgrade, until it has been joined.
    if let Err(error) = receiver
        .register(owner, move |session| async move {
            let transfer = async {
                let upstream = session.run(connect(&registered_generation, &display)).await;
                let Ok(Ok(upstream)) = upstream else {
                    return;
                };
                if ready_tx.send(()).is_err() {
                    return;
                }
                let Ok(Ok(socket)) = session.run(socket_rx).await else {
                    return;
                };
                relay(socket, upstream, session).await;
            };
            tokio::select! {
                biased;
                () = registered_generation.invalidated() => {}
                () = transfer => {}
            }
        })
        .await
    {
        return runtime_failure(error);
    }
    let ready = tokio::select! {
        biased;
        () = generation.invalidated() => false,
        result = ready_rx => result.is_ok(),
    };
    if !ready || !generation.active() {
        return unavailable(true);
    }
    ws.max_message_size(8 * 1024 * 1024)
        .max_frame_size(8 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let _ = socket_tx.send(socket);
        })
        .into_response()
}

fn unavailable(retryable: bool) -> Response {
    let mut response = protocol_failure(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "desktop_unavailable",
        "the desktop display service is unavailable",
        retryable,
    );
    if retryable {
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("1"),
        );
    } else {
        response.headers_mut().insert(
            DESKTOP_STATE_HEADER,
            axum::http::HeaderValue::from_static(DESKTOP_STATE_FAILED),
        );
    }
    response
}

fn ownership(
    headers: &axum::http::HeaderMap,
) -> crate::failure::Result<Option<layer3_adapter::DesktopOwnership>> {
    let mut values = headers
        .get_all(layer3_adapter::desktop_ownership::wire::DESKTOP_OWNER_HEADER)
        .iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() || value.len() > 4096 {
        return Err(invalid());
    }
    let owner: layer3_adapter::DesktopOwnership =
        serde_json::from_slice(value.as_bytes()).map_err(|_| invalid())?;
    owner.validate().map_err(|_| invalid())?;
    Ok(Some(owner))
}

fn invalid() -> crate::failure::RuntimeError {
    crate::failure::RuntimeError::rejected(
        "desktop_ownership_stale",
        "desktop ownership header is invalid",
    )
}

#[cfg(test)]
mod tests;

async fn relay(socket: WebSocket, upstream: Display, session: Session) {
    let (mut upstream_sink, mut upstream_source) = upstream.split();
    let (mut sink, mut source) = socket.split();
    loop {
        tokio::select! {
            incoming = session.run(source.next()) => match incoming {
                Ok(Some(Ok(axum::extract::ws::Message::Binary(bytes)))) => {
                    if !matches!(session.run(upstream_sink.send(TungsteniteMessage::Binary(bytes.to_vec()))).await, Ok(Ok(()))) { return; }
                }
                Ok(Some(Ok(axum::extract::ws::Message::Text(text)))) => {
                    if !matches!(session.run(upstream_sink.send(TungsteniteMessage::Text(text.to_string()))).await, Ok(Ok(()))) { return; }
                }
                Ok(Some(Ok(axum::extract::ws::Message::Ping(bytes)))) => {
                    if !matches!(session.run(upstream_sink.send(TungsteniteMessage::Ping(bytes.to_vec()))).await, Ok(Ok(()))) { return; }
                }
                Ok(Some(Ok(axum::extract::ws::Message::Close(_)))) | Ok(None) | Err(_) => return,
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(_))) => return,
            },
            outgoing = session.run(upstream_source.next()) => match outgoing {
                Ok(Some(Ok(TungsteniteMessage::Binary(bytes)))) => {
                    if !matches!(session.run(sink.send(axum::extract::ws::Message::Binary(bytes.to_vec().into()))).await, Ok(Ok(()))) { return; }
                }
                Ok(Some(Ok(TungsteniteMessage::Text(text)))) => {
                    if !matches!(session.run(sink.send(axum::extract::ws::Message::Text(text.to_string().into()))).await, Ok(Ok(()))) { return; }
                }
                Ok(Some(Ok(TungsteniteMessage::Ping(bytes)))) => {
                    if !matches!(session.run(sink.send(axum::extract::ws::Message::Ping(bytes.to_vec().into()))).await, Ok(Ok(()))) { return; }
                }
                Ok(Some(Ok(TungsteniteMessage::Pong(_)))) => {}
                Ok(Some(Ok(TungsteniteMessage::Close(_)))) | Ok(None) | Err(_) => return,
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(_))) => return,
            },
        }
    }
}
