//! Authenticated relay to the runtime-local noVNC WebSocket.
//!
//! The runtime never exposes port 6080 directly.  The supervisor's control
//! middleware protects this endpoint, and a higher layer may port-forward this
//! same authenticated stream through CMA.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ws::WebSocket, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use super::protocol_failure;
use crate::supervisor::Supervisor;

pub(super) async fn websocket(
    ws: WebSocketUpgrade,
    State(supervisor): State<Arc<Supervisor>>,
) -> Response {
    let Some(token) = supervisor.desktop_websocket_token() else {
        return protocol_failure(
            axum::http::StatusCode::NOT_FOUND,
            "desktop_unavailable",
            "this Sandbox was not provisioned with a desktop machine",
            false,
        );
    };
    let config = WebSocketConfig {
        max_message_size: Some(8 * 1024 * 1024),
        max_frame_size: Some(8 * 1024 * 1024),
        ..WebSocketConfig::default()
    };
    let mut request = "ws://127.0.0.1:6080/websockify"
        .into_client_request()
        .expect("constant desktop URL is valid");
    // websockify's host-token mode keeps the boot credential out of request
    // paths and access logs. Browser pages cannot set this private Host value.
    let mut credential: axum::http::HeaderValue =
        token.parse().expect("generated desktop token is valid");
    credential.set_sensitive(true);
    request.headers_mut().insert("host", credential);
    let Ok(Ok((upstream, _))) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    else {
        return protocol_failure(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "desktop_unavailable",
            "the desktop display service is unavailable",
            true,
        );
    };
    ws.max_message_size(8 * 1024 * 1024)
        .max_frame_size(8 * 1024 * 1024)
        .on_upgrade(move |socket| relay(socket, upstream))
        .into_response()
}

async fn relay(
    socket: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (mut upstream_sink, mut upstream_source) = upstream.split();
    let (mut sink, mut source) = socket.split();
    loop {
        tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(axum::extract::ws::Message::Binary(bytes))) => {
                    if upstream_sink.send(TungsteniteMessage::Binary(bytes.to_vec())).await.is_err() { return; }
                }
                Some(Ok(axum::extract::ws::Message::Text(text))) => {
                    if upstream_sink.send(TungsteniteMessage::Text(text.to_string())).await.is_err() { return; }
                }
                Some(Ok(axum::extract::ws::Message::Ping(bytes))) => {
                    if upstream_sink.send(TungsteniteMessage::Ping(bytes.to_vec())).await.is_err() { return; }
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
