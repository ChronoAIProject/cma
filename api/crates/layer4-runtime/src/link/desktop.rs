//! Authenticated relay to the runtime-local noVNC WebSocket.
//!
//! The runtime never exposes port 6080 directly.  The supervisor's control
//! middleware protects this endpoint, and a higher layer may port-forward this
//! same authenticated stream through CMA.

use std::sync::Arc;

use axum::extract::{ws::WebSocket, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use super::protocol_failure;
use crate::supervisor::Supervisor;

pub(super) async fn websocket(
    ws: WebSocketUpgrade,
    State(supervisor): State<Arc<Supervisor>>,
) -> Response {
    if !supervisor.desktop_enabled() {
        return protocol_failure(
            axum::http::StatusCode::NOT_FOUND,
            "desktop_unavailable",
            "this Sandbox was not provisioned with a desktop machine",
            false,
        );
    }
    ws.on_upgrade(relay).into_response()
}

async fn relay(socket: WebSocket) {
    let Ok((upstream, _)) =
        tokio_tungstenite::connect_async("ws://127.0.0.1:6080/websockify").await
    else {
        return;
    };
    let (mut upstream_sink, mut upstream_source) = upstream.split();
    let (mut sink, mut source) = socket.split();
    loop {
        tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(axum::extract::ws::Message::Binary(bytes))) => {
                    if upstream_sink.send(TungsteniteMessage::Binary(bytes.to_vec().into())).await.is_err() { return; }
                }
                Some(Ok(axum::extract::ws::Message::Text(text))) => {
                    if upstream_sink.send(TungsteniteMessage::Text(text.to_string().into())).await.is_err() { return; }
                }
                Some(Ok(axum::extract::ws::Message::Ping(bytes))) => {
                    if upstream_sink.send(TungsteniteMessage::Ping(bytes.to_vec().into())).await.is_err() { return; }
                }
                Some(Ok(axum::extract::ws::Message::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            },
            outgoing = upstream_source.next() => match outgoing {
                Some(Ok(TungsteniteMessage::Binary(bytes))) => {
                    if sink.send(axum::extract::ws::Message::Binary(bytes.to_vec().into())).await.is_err() { return; }
                }
                Some(Ok(TungsteniteMessage::Text(text))) => {
                    if sink.send(axum::extract::ws::Message::Text(text.to_string().into())).await.is_err() { return; }
                }
                Some(Ok(TungsteniteMessage::Ping(bytes))) => {
                    if sink.send(axum::extract::ws::Message::Ping(bytes.to_vec().into())).await.is_err() { return; }
                }
                Some(Ok(TungsteniteMessage::Pong(_))) => {}
                Some(Ok(TungsteniteMessage::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            },
        }
    }
}
