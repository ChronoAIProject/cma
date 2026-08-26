//! The conversation-plane half of a runtime link.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::bridge::Outbound;
use crate::lanes::{self, LANE_CONVERSATION, LANE_CONVERSATION_GAP};
use crate::link::{control, Sink, CONTROL_REPLIES};
use crate::AppState;

/// Drive the conversation plane.
///
/// Frames cross in both directions without being read. What this pod does not
/// interpret cannot break when a kernel version changes what a frame looks
/// like, which is the same bargain the conversation capture makes.
pub(crate) async fn relay_conversation(
    mut sink: Sink,
    mut source: futures_util::stream::SplitStream<WebSocket>,
    state: AppState,
    id: String,
) -> std::io::Result<()> {
    let bridge = state.conversation_bridge().await?;

    // Subscribed before the backlog is taken, so a frame produced between the
    // two is delivered live rather than falling between them.
    let mut live = bridge.subscribe();
    let backlog = bridge.backlog().await;
    if !backlog.is_empty() {
        tracing::info!(runtime = %id, frames = backlog.len(), "replaying what was said while detached");
    }
    for held in backlog {
        if sink.send(outbound(held)).await.is_err() {
            return Ok(());
        }
    }

    // As on the terminal plane: one sink, and control answers ride it too.
    let (replies, mut answers) = mpsc::channel::<String>(CONTROL_REPLIES);
    let runtime = id.clone();
    let mut pump_out = tokio::spawn(async move {
        loop {
            let said = tokio::select! {
                answer = answers.recv() => {
                    match answer {
                        Some(text) => {
                            if sink.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        None => continue,
                    }
                }
                said = live.recv() => said,
            };
            match said {
                Ok(said) => {
                    if sink.send(outbound(said)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // Unlike a screen, a conversation cannot be re-derived, so
                    // a lagging consumer leaves a real hole. Said as loudly as
                    // one, and marked in the stream where it happened.
                    tracing::error!(runtime = %runtime, skipped, "conversation consumer lagged");
                    if sink
                        .send(gap(usize::try_from(skipped).unwrap_or(usize::MAX)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let state_in = state.clone();
    let runtime = id.clone();
    let bridge_in = std::sync::Arc::clone(&bridge);
    let mut pump_in = tokio::spawn(async move {
        while let Some(frame) = source.next().await {
            match frame {
                Ok(Message::Binary(bytes)) => match lanes::unframe(&bytes) {
                    Some((LANE_CONVERSATION, payload)) => {
                        let _admission = state_in.startup_gate.read().await;
                        if !bridge_in.send(payload.to_vec()).await {
                            tracing::warn!(runtime = %runtime, "the bridge stopped accepting frames");
                            break;
                        }
                    }
                    Some((lane, _)) => {
                        tracing::warn!(runtime = %runtime, lane, "frame on a lane this link does not carry")
                    }
                    None => {}
                },
                Ok(Message::Text(text)) => {
                    control(&text, &runtime, &state_in, None, &replies).await
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(runtime = %runtime, error = %e, "conversation read failed");
                    break;
                }
            }
        }
    });

    // A bridge that exits is the runtime being lost, not a stream that went
    // quiet, and has to end the link so the control plane finds out.
    tokio::select! {
        _ = &mut pump_out => pump_in.abort(),
        _ = &mut pump_in => pump_out.abort(),
        _ = bridge.ended() => {
            tracing::warn!(runtime = %id, "the conversation bridge ended; closing the link");
            pump_out.abort();
            pump_in.abort();
        }
    }
    /*
     * If the model endpoint refused this pod while the conversation ran, that
     * is very likely why it ended — an agent that cannot reach its model
     * stops having anything to say. Said here, in the pod's own log, with
     * the status the proxy actually saw.
     *
     * The control plane learns the same fact from its own side: the link
     * closing fails every call waiting on it, and the turn is reported as a
     * failure rather than as an empty ending.
     */
    if let Some(fault) = state.model_fault.get() {
        tracing::warn!(runtime = %id, %fault, "the model endpoint had refused this runtime");
    }
    tracing::info!(runtime = %id, "conversation detached");
    Ok(())
}

/// One thing the bridge said, on the lane that says what it is.
pub(crate) fn outbound(said: Outbound) -> Message {
    match said {
        Outbound::Frame(bytes) => Message::Binary(lanes::frame(LANE_CONVERSATION, &bytes).into()),
        Outbound::Gap { frames } => gap(frames),
    }
}

/// A boundary where frames were dropped rather than kept.
///
/// Carried in the conversation's place so the control plane records where the
/// gap falls, not merely that one exists.
pub(crate) fn gap(frames: usize) -> Message {
    let payload = serde_json::json!({ "frames": frames }).to_string();
    Message::Binary(lanes::frame(LANE_CONVERSATION_GAP, payload.as_bytes()).into())
}
