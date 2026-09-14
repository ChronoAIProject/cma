use std::sync::Arc;

use axum::extract::{
    ws::{Message, WebSocket},
    Path, State, WebSocketUpgrade,
};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};

use super::protocol_failure;
use crate::bridge::{Bridge, Outbound, MAX_FRAME_BYTES};
use crate::lanes::{self, Ready};
use crate::supervisor::Supervisor;

pub(super) async fn conversation(
    ws: WebSocketUpgrade,
    Path(agent_id): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
) -> Response {
    let Some(bridge) = supervisor.bridge(&agent_id).await else {
        return protocol_failure(
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent process was not found",
            false,
        );
    };
    ws.max_message_size(MAX_FRAME_BYTES + 1)
        .max_frame_size(MAX_FRAME_BYTES + 1)
        .on_upgrade(move |socket| relay_conversation(socket, bridge, agent_id))
        .into_response()
}

async fn relay_conversation(mut socket: WebSocket, bridge: Arc<Bridge>, agent_id: String) {
    let mut live = bridge.subscribe();
    let backlog = bridge.backlog().await;
    let ready = match serde_json::to_string(&Ready::agent(&agent_id)) {
        Ok(ready) => ready,
        Err(error) => {
            tracing::error!(%error, %agent_id, "conversation ready frame could not be encoded");
            return;
        }
    };
    if socket.send(Message::Text(ready.into())).await.is_err() {
        return;
    }
    for held in backlog {
        if socket.send(outbound(held)).await.is_err() {
            return;
        }
    }

    let (sink, mut source) = socket.split();
    let output_bridge = Arc::clone(&bridge);
    let mut output = tokio::spawn(async move {
        pump_output(sink, &mut live, output_bridge, &agent_id).await;
    });
    loop {
        tokio::select! {
            result = &mut output => {
                if let Err(error) = result {
                    tracing::warn!(%error, "conversation output task failed");
                }
                return;
            }
            incoming = source.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match lanes::unframe(&bytes) {
                    Some((lanes::CONVERSATION, payload))
                        if bridge.send(payload.to_vec()).await => {}
                    Some((lanes::CONVERSATION, _)) => {
                        tracing::warn!("conversation bridge stopped accepting input");
                        output.abort();
                        return;
                    }
                    Some((lane, _)) => {
                        tracing::warn!(lane, "frame arrived on an unsupported conversation lane");
                    }
                    None => {}
                },
                Some(Ok(Message::Close(_))) | None => {
                    output.abort();
                    return;
                }
                Some(Ok(Message::Text(_))) => {
                    tracing::warn!("text frame ignored after the conversation ready frame");
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    tracing::warn!(%error, "conversation WebSocket input failed");
                    output.abort();
                    return;
                }
            }
        }
    }
}

async fn pump_output(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    live: &mut tokio::sync::broadcast::Receiver<Outbound>,
    bridge: Arc<Bridge>,
    agent_id: &str,
) {
    loop {
        let said = tokio::select! {
            _ = bridge.ended() => break,
            said = live.recv() => said,
        };
        let message = match said {
            Ok(said) => outbound(said),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::error!(%agent_id, skipped, "conversation consumer lagged");
                gap(usize::try_from(skipped).unwrap_or(usize::MAX))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if sink.send(message).await.is_err() {
            return;
        }
    }
    let _ = sink.send(Message::Close(None)).await;
}

fn outbound(said: Outbound) -> Message {
    match said {
        Outbound::Frame(bytes) => Message::Binary(lanes::frame(lanes::CONVERSATION, &bytes).into()),
        Outbound::Gap { frames } => gap(frames),
    }
}

fn gap(frames: usize) -> Message {
    let payload = serde_json::json!({ "frames": frames }).to_string();
    Message::Binary(lanes::frame(lanes::CONVERSATION_GAP, payload.as_bytes()).into())
}
