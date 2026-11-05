use std::sync::Arc;

use axum::extract::{
    ws::{Message, WebSocket},
    Path, Query, State, WebSocketUpgrade,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};

use super::protocol_failure;
use crate::bridge::{Bridge, Outbound, MAX_FRAME_BYTES};
use crate::lanes::{self, Ready};
use crate::manager_ownership::OwnerSession;
use crate::supervisor::Supervisor;

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationQuery {
    output_ack: Option<String>,
}

pub(super) async fn conversation(
    ws: WebSocketUpgrade,
    Path(agent_id): Path<String>,
    Query(query): Query<ConversationQuery>,
    State(supervisor): State<Arc<Supervisor>>,
    headers: HeaderMap,
) -> Response {
    let owner = match ownership_header(&headers) {
        Ok(owner) => owner,
        Err(error) => return super::runtime_failure(error),
    };
    let authority = match supervisor.manager_ownership.session(&agent_id, owner).await {
        Ok(authority) => authority,
        Err(error) => return super::runtime_failure(error),
    };
    let Some(bridge) = supervisor.bridge(&agent_id).await else {
        return protocol_failure(
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent process was not found",
            false,
        );
    };
    let acknowledge_output =
        match bridge.select_output_acknowledgement(query.output_ack.as_deref() == Some("1")) {
            Ok(acknowledge_output) => acknowledge_output,
            Err(()) => {
                return protocol_failure(
                    StatusCode::CONFLICT,
                    "output_acknowledgement_required",
                    "this agent conversation requires durable output acknowledgement",
                    false,
                );
            }
        };
    ws.max_message_size(MAX_FRAME_BYTES + 1 + lanes::OUTPUT_IDENTITY_BYTES)
        .max_frame_size(MAX_FRAME_BYTES + 1 + lanes::OUTPUT_IDENTITY_BYTES)
        .on_upgrade(move |socket| {
            relay_conversation(socket, bridge, agent_id, acknowledge_output, authority)
        })
        .into_response()
}

async fn relay_conversation(
    mut socket: WebSocket,
    bridge: Arc<Bridge>,
    agent_id: String,
    acknowledge_output: bool,
    authority: OwnerSession,
) {
    // Subscribe before the first snapshot; cursor filtering makes notifications
    // observed on both sides of a snapshot harmless.
    let reliable_live = acknowledge_output.then(|| bridge.retained_output.subscribe());
    let legacy_live = (!acknowledge_output).then(|| bridge.subscribe());
    let ready = if acknowledge_output {
        Ready::agent(&agent_id).with_acknowledgement(bridge.retained_output.stream_id())
    } else {
        Ready::agent(&agent_id)
    }
    .with_owner(authority.owner());
    let ready = match serde_json::to_string(&ready) {
        Ok(ready) => ready,
        Err(error) => {
            tracing::error!(%error, %agent_id, "conversation ready frame could not be encoded");
            return;
        }
    };
    if !send_owned(&mut socket, Message::Text(ready.into()), &authority).await {
        return;
    }
    if !acknowledge_output {
        for held in bridge.backlog().await {
            if !send_owned(&mut socket, outbound(held), &authority).await {
                return;
            }
        }
    }

    let (sink, mut source) = socket.split();
    let output_bridge = Arc::clone(&bridge);
    let output_authority = authority.clone();
    let mut output = tokio::spawn(async move {
        if let Some(live) = reliable_live {
            pump_reliable_output(sink, live, output_bridge, &agent_id, output_authority).await;
        } else if let Some(mut live) = legacy_live {
            pump_output(sink, &mut live, output_bridge, &agent_id, output_authority).await;
        }
    });
    loop {
        tokio::select! {
            biased;
            _ = authority.expired() => { output.abort(); return; }
            result = &mut output => {
                if let Err(error) = result {
                    tracing::warn!(%error, "conversation output task failed");
                }
                return;
            }
            incoming = source.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match lanes::unframe(&bytes) {
                    Some((lanes::OUTPUT_ACKNOWLEDGEMENT, payload)) if acknowledge_output => {
                        let Ok(_permit) = authority.permit().await else { output.abort(); return; };
                        let accepted = match lanes::output_acknowledgement(payload) {
                            Some((stream_id, cursor)) => {
                                bridge.retained_output.acknowledge(stream_id, cursor).await
                            }
                            None => false,
                        };
                        if !accepted {
                            tracing::warn!("invalid conversation output acknowledgement");
                            output.abort();
                            return;
                        }
                    }
                    Some((lanes::CONVERSATION, payload)) => {
                        if authority.permit().await.is_err() || !bridge.send_owned(payload.to_vec(), authority.clone()).await {
                            output.abort();
                            return;
                        }
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

async fn pump_reliable_output(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut live: tokio::sync::watch::Receiver<()>,
    bridge: Arc<Bridge>,
    agent_id: &str,
    authority: OwnerSession,
) {
    let mut sent_cursor = 0;
    loop {
        let snapshot = match bridge.retained_output.snapshot(sent_cursor).await {
            Ok(snapshot) => snapshot,
            Err(frames) => {
                tracing::error!(%agent_id, frames, "conversation output retention exhausted");
                let _ = send_owned(&mut sink, gap(frames), &authority).await;
                break;
            }
        };
        for frame in snapshot.frames {
            let bytes = lanes::acknowledged_frame(
                bridge.retained_output.stream_id(),
                frame.cursor,
                &frame.bytes,
            );
            if !send_owned(&mut sink, Message::Binary(bytes.into()), &authority).await {
                return;
            }
            sent_cursor = frame.cursor;
        }
        // Wait for stdout to drain and its final frame to be acknowledged before
        // closing. Process exit alone can race the stdout reader's final bytes.
        if snapshot.finished_and_acknowledged || live.changed().await.is_err() {
            break;
        }
    }
    let _ = send_owned(&mut sink, Message::Close(None), &authority).await;
}

async fn pump_output(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    live: &mut tokio::sync::broadcast::Receiver<Outbound>,
    bridge: Arc<Bridge>,
    agent_id: &str,
    authority: OwnerSession,
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
        if !send_owned(&mut sink, message, &authority).await {
            return;
        }
    }
    let _ = send_owned(&mut sink, Message::Close(None), &authority).await;
}

async fn send_owned<S>(sink: &mut S, message: Message, authority: &OwnerSession) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    let Ok(permit) = authority.permit().await else {
        return false;
    };
    if authority.owner().is_none() {
        return sink.send(message).await.is_ok();
    }
    let bounded = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let until = permit
        .deadline()
        .map_or(bounded, |deadline| deadline.min(bounded));
    tokio::time::timeout_at(until, sink.send(message))
        .await
        .is_ok_and(|result| result.is_ok())
}

pub(super) fn ownership_header(
    headers: &HeaderMap,
) -> crate::failure::Result<Option<layer3_adapter::owner_protocol::TransportOwner>> {
    let mut values = headers
        .get_all(layer3_adapter::owner_protocol::OWNER_HEADER)
        .iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let invalid = || {
        crate::failure::RuntimeError::rejected(
            "manager_ownership_invalid",
            "invalid ownership header",
        )
    };
    if values.next().is_some() {
        return Err(invalid());
    }
    layer3_adapter::owner_protocol::TransportOwner::from_header(
        value.to_str().map_err(|_| invalid())?,
    )
    .map(Some)
    .map_err(|_| invalid())
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
