//! The link between this pod and its control plane.
//!
//! One WebSocket, carrying whichever lanes the session on it needs: control
//! always, and then either the terminal or the conversation. A session is one
//! or the other — that is what the control plane granted it — so a pod never
//! drives both, and refuses to try.
//!
//! Control travelling alongside is the point of the arrangement. The
//! credential's consumer has always been the loopback proxy in this pod, and
//! it rode the terminal channel only because the terminal was the sole channel
//! there was. A kernel that renders no terminal could not have been told its
//! credential had changed at all.
//!
//! ## Two dialects, and why both are spoken
//!
//! CLAUDE.md §5 says no rollout interrupts a session its owner has not
//! stopped, and a rollout has both halves in flight at once: pods that predate
//! lanes running against a control plane that knows them, and the reverse.
//!
//! So the dialect is settled per link, by the control plane, in the query it
//! opens with. A control plane that says nothing gets exactly what it got
//! before — unprefixed binary, terminal bytes both ways — because that is what
//! it will understand. One that asks for the lane wire gets a discriminator on
//! every binary frame. Neither has to guess, and neither can be wrong.
//!
//! The pod answers with a ready frame naming its wire and its lanes. An old
//! pod sends nothing of the kind, which is how a control plane learns it is
//! talking to one.
//!
//! The route is still called `terminal`. It carries more than one now, and the
//! name is kept because pods and control planes address each other by it, and
//! renaming an address to describe it better is not worth a rollout.

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use serde::Deserialize;

use crate::bridge::Outbound;
use crate::lanes::{self, Ready, LANE_CONVERSATION, LANE_CONVERSATION_GAP, LANE_TERMINAL};
use crate::pty::PtySession;
use crate::upstream::PLACEHOLDER_KEY;
use crate::AppState;

/// What the control plane asks for when it opens a link.
#[derive(Debug, Deserialize)]
pub(crate) struct AttachParams {
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    /// The wire the control plane will speak on this link.
    ///
    /// Absent means the one that existed before lanes did. Settled by the
    /// caller rather than negotiated, because a pod cannot tell a legacy
    /// terminal byte from a lane discriminator by looking at it, and a wrong
    /// guess writes garbage into somebody's session.
    #[serde(default)]
    wire: Option<u32>,
    /// Which data lane this link is for: `term` or `conv`.
    ///
    /// Absent means the terminal, which is what every link was.
    #[serde(default)]
    lanes: Option<String>,
}

impl AttachParams {
    /// Whether this link carries lane discriminators.
    fn framed(&self) -> bool {
        self.wire.is_some_and(|v| v >= lanes::WIRE_VERSION)
    }

    /// Whether this link drives the conversation rather than the terminal.
    fn conversational(&self) -> bool {
        self.lanes.as_deref() == Some("conv")
    }
}

/// A control message from the control plane.
///
/// The control lane, which is what the text-frame vocabulary always was. Data
/// travels as binary; this is everything else.
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub(crate) enum ControlFrame {
    Resize {
        cols: u16,
        rows: u16,
    },
    Signal {
        sig: String,
    },
    /// Replace the credential the local proxy presents upstream.
    ///
    /// On the control lane, which is where it belongs: its consumer is this
    /// pod's proxy, and a kernel with no terminal still calls out on its
    /// owner's behalf.
    Credential {
        value: String,
    },
    /// Restart the supervised program under a different front-end.
    ///
    /// The command must be one this pod was told at boot it may run; anything
    /// else is refused by name.
    SwitchPlane {
        command: String,
    },
}

type Sink = SplitSink<WebSocket, Message>;

pub(crate) async fn attach(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    Query(params): Query<AttachParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = relay(socket, state, id.clone(), params).await {
            tracing::warn!(runtime = %id, error = %e, "runtime link ended with an error");
        }
    })
}

async fn relay(
    socket: WebSocket,
    state: AppState,
    id: String,
    params: AttachParams,
) -> std::io::Result<()> {
    let (mut sink, source) = socket.split();

    // Sent before anything else, so a control plane can decide how to read
    // every frame that follows from the first one it receives. A pod that
    // predates this sends nothing here, which is exactly how it is recognised.
    if params.framed() {
        let lanes = if params.conversational() {
            ["ctl", "conv"]
        } else {
            ["ctl", "term"]
        };
        let ready = serde_json::to_string(&Ready::declaring(&lanes).resuming(state.resuming()))
            .map_err(|e| std::io::Error::other(format!("cannot render the ready frame: {e}")))?;
        if sink.send(Message::Text(ready.into())).await.is_err() {
            return Ok(());
        }
    }

    if params.conversational() {
        relay_conversation(sink, source, state, id).await
    } else {
        relay_terminal(sink, source, state, id, params).await
    }
}

/// Drive the terminal plane.
///
/// Byte for byte what it always was, one discriminator further along when the
/// control plane asked for lanes.
async fn relay_terminal(
    mut sink: Sink,
    mut source: futures_util::stream::SplitStream<WebSocket>,
    state: AppState,
    id: String,
    params: AttachParams,
) -> std::io::Result<()> {
    let cols = params.cols.unwrap_or(80).max(1);
    let rows = params.rows.unwrap_or(24).max(1);
    let framed = params.framed();

    let session: std::sync::Arc<PtySession> = state
        .registry
        .get_or_start(&id, &state.agent_command(), cols, rows, PLACEHOLDER_KEY)
        .await?;

    // Subscribe before the first resize so no output produced during startup
    // is missed by this attachment.
    let mut output = session.output.subscribe();
    session.resize(cols, rows)?;

    let runtime = id.clone();
    let mut pump_out = tokio::spawn(async move {
        loop {
            match output.recv().await {
                Ok(chunk) => {
                    let payload = if framed {
                        lanes::frame(LANE_TERMINAL, &chunk)
                    } else {
                        chunk
                    };
                    if sink.send(Message::Binary(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // The consumer can always recover from a fresh screen, so
                    // lagging is survivable and must be reported, not hidden.
                    tracing::warn!(runtime = %runtime, skipped, "terminal consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let session_in = std::sync::Arc::clone(&session);
    let state_in = state.clone();
    let runtime = id.clone();
    let mut pump_in = tokio::spawn(async move {
        while let Some(frame) = source.next().await {
            match frame {
                Ok(Message::Binary(bytes)) => {
                    let typed = if framed {
                        match lanes::unframe(&bytes) {
                            Some((LANE_TERMINAL, payload)) => payload.to_vec(),
                            Some((lane, _)) => {
                                // A lane this link does not serve. Ignored and
                                // named rather than written into the pty,
                                // where it would appear as typed rubbish.
                                tracing::warn!(runtime = %runtime, lane, "frame on a lane this link does not carry");
                                continue;
                            }
                            None => continue,
                        }
                    } else {
                        bytes.to_vec()
                    };
                    if let Err(e) = session_in.write_input(&typed) {
                        tracing::warn!(runtime = %runtime, error = %e, "pty write failed");
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    control(&text, &runtime, &state_in, Some(&session_in)).await
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(runtime = %runtime, error = %e, "terminal read failed");
                    break;
                }
            }
        }
    });

    // A dropped connection detaches; it does not end the session. The process
    // keeps running and the next attach reuses it.
    tokio::select! {
        _ = &mut pump_out => pump_in.abort(),
        _ = &mut pump_in => pump_out.abort(),
    }
    tracing::info!(runtime = %id, "terminal detached");
    Ok(())
}

/// Drive the conversation plane.
///
/// Frames cross in both directions without being read. What this pod does not
/// interpret cannot break when a kernel version changes what a frame looks
/// like, which is the same bargain the conversation capture makes.
async fn relay_conversation(
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

    let runtime = id.clone();
    let mut pump_out = tokio::spawn(async move {
        loop {
            match live.recv().await {
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
                Ok(Message::Text(text)) => control(&text, &runtime, &state_in, None).await,
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
    tracing::info!(runtime = %id, "conversation detached");
    Ok(())
}

/// One thing the bridge said, on the lane that says what it is.
fn outbound(said: Outbound) -> Message {
    match said {
        Outbound::Frame(bytes) => Message::Binary(lanes::frame(LANE_CONVERSATION, &bytes).into()),
        Outbound::Gap { frames } => gap(frames),
    }
}

/// A boundary where frames were dropped rather than kept.
///
/// Carried in the conversation's place so the control plane records where the
/// gap falls, not merely that one exists.
fn gap(frames: usize) -> Message {
    let payload = serde_json::json!({ "frames": frames }).to_string();
    Message::Binary(lanes::frame(LANE_CONVERSATION_GAP, payload.as_bytes()).into())
}

/// Act on a control-lane frame.
///
/// Shared by both planes, which is the whole reason the lane exists: a
/// credential reaches a conversation session the same way it reaches a
/// terminal one. Resize and signal need a terminal, and say so rather than
/// pretending when there is none.
async fn control(
    text: &str,
    runtime: &str,
    state: &AppState,
    session: Option<&std::sync::Arc<PtySession>>,
) {
    match serde_json::from_str::<ControlFrame>(text) {
        Ok(ControlFrame::Credential { value }) => {
            // Never logged with its value; the holder reports only that a
            // replacement happened.
            state.credential.set(value).await;
        }
        Ok(ControlFrame::SwitchPlane { command }) => {
            // The program is swapped in place — the pod, its filesystem and
            // its conversation stay. The allowlist is checked in `switch_command`.
            state.switch_command(&command);
        }
        Ok(ControlFrame::Resize { cols, rows }) => match session {
            Some(session) => {
                if let Err(e) = session.resize(cols, rows) {
                    tracing::warn!(runtime, error = %e, "resize rejected");
                }
            }
            None => tracing::debug!(runtime, "resize ignored: this link has no terminal"),
        },
        Ok(ControlFrame::Signal { sig }) => {
            // Signals are delivered in band as the control character the
            // kernel already understands, so no separate signalling path is
            // needed.
            let byte: Option<u8> = match sig.as_str() {
                "INT" => Some(0x03),
                "QUIT" => Some(0x1c),
                "TERM" | "HUP" => Some(0x04),
                _ => None,
            };
            match (byte, session) {
                (Some(b), Some(session)) => {
                    if let Err(e) = session.write_input(&[b]) {
                        tracing::warn!(runtime, error = %e, "signal write failed");
                    }
                }
                (Some(_), None) => {
                    tracing::debug!(runtime, %sig, "signal ignored: this link has no terminal")
                }
                (None, _) => tracing::warn!(runtime, %sig, "unknown signal ignored"),
            }
        }
        Err(e) => tracing::warn!(runtime, error = %e, "malformed control frame ignored"),
    }
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod link_tests;
