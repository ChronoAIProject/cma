use super::*;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{tungstenite::Message as Upstream, MaybeTlsStream, WebSocketStream};

pub(super) struct Active(pub(super) Arc<StateData>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) async fn run(
    mut socket: WebSocket,
    mut upstream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    active: Active,
) {
    let mut stop = active.0.stop.subscribe();
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            message = socket.next() => {
                let Some(Ok(message)) = message else { break; };
                let message = match message {
                    Message::Binary(bytes) => Upstream::Binary(bytes.to_vec()),
                    Message::Text(text) => Upstream::Text(text.to_string()),
                    Message::Ping(bytes) => Upstream::Ping(bytes.to_vec()),
                    Message::Pong(bytes) => Upstream::Pong(bytes.to_vec()),
                    Message::Close(_) => break,
                };
                if upstream.send(message).await.is_err() { break; }
            }
            message = upstream.next() => {
                let Some(Ok(message)) = message else { break; };
                let message = match message {
                    Upstream::Binary(bytes) => Message::Binary(bytes.into()),
                    Upstream::Text(text) => Message::Text(text.into()),
                    Upstream::Ping(bytes) => Message::Ping(bytes.into()),
                    Upstream::Pong(bytes) => Message::Pong(bytes.into()),
                    Upstream::Close(_) | Upstream::Frame(_) => break,
                };
                if socket.send(message).await.is_err() { break; }
            }
        }
    }
    // Both owned links are dropped together. The actual receiver independently
    // enforces expiry/revocation even if the client or proxy no longer reads.
}
