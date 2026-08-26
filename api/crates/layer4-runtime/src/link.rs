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
        ws::{close_code, CloseFrame, Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::lanes::{self, Ready, LANE_TERMINAL};
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
    /// How often, in milliseconds, to send a resource reading on this link.
    ///
    /// Present makes this a telemetry link and nothing else: no pty is taken,
    /// no geometry is declared, no conversation is bridged. Asked for per link
    /// rather than baked into the image, because the image is the runtime
    /// version and changing it rotates every live session.
    #[serde(default)]
    telemetry: Option<u64>,
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

    /// Whether this link carries readings rather than a plane.
    fn telemetric(&self) -> bool {
        self.telemetry.is_some()
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
        /// Absent on legacy frames. New callers receive this exact value back.
        #[serde(default)]
        operation_id: Option<String>,
        value: String,
    },
    /// Install one authenticated, launch-scoped runtime policy or tombstone.
    ///
    /// The supervisor treats the envelope as policy data only. Signature,
    /// audience and monotonic fencing are checked by the Layer 4 evaluator
    /// before it can affect the credential-attaching proxy.
    GithubCapability {
        operation_id: String,
        envelope: String,
    },
    /// Restart the supervised program under a different front-end.
    ///
    /// The command must be one this pod was told at boot it may run; anything
    /// else is refused by name.
    SwitchPlane {
        command: String,
    },
}

/// How many unsent control answers a link will hold.
///
/// Small on purpose: answers are one per request and are consumed at once, so
/// a backlog here would mean the socket is already gone.
pub(crate) const CONTROL_REPLIES: usize = 4;

/// The tag a supervisor answers a plane switch with.
///
/// Named here and matched by the adapter: the two ends have to agree on one
/// word, and a literal repeated in two crates is how they stop agreeing.
pub(crate) const SWITCH_RESULT: &str = "switch_plane_result";

/// The tag returned after a correlated credential was atomically installed.
pub(crate) const CREDENTIAL_RESULT: &str = "credential_result";

/// The tag returned after a signed GitHub policy/tombstone was settled.
pub(crate) const GITHUB_CAPABILITY_RESULT: &str = "github_capability_result";

/// A process ending sent to the control plane after its terminal is drained.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum OutboundControl {
    Exit { code: Option<i32> },
}

/// Why the socket's inbound half stopped.
///
/// A peer ending means there is nowhere left to send output. A PTY write
/// failure is different: the child may just have closed its slave, and the
/// output half still owes the peer the latched process ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalInputEnd {
    PeerEnded,
    PtyWriteFailed,
}

/// The exact terminal byte represented by each supported signal name.
fn signal_byte(sig: &str) -> Option<u8> {
    match sig {
        "INT" => Some(0x03),
        "QUIT" => Some(0x1c),
        "TERM" | "HUP" => Some(0x04),
        _ => None,
    }
}

pub(crate) type Sink = SplitSink<WebSocket, Message>;

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

    if params.telemetric() && !params.framed() {
        tracing::warn!(runtime = %id, "a resource reading cannot be sent on a wire with no lane discriminator");
        let _ = sink.send(Message::Close(None)).await;
        return Ok(());
    }

    // Sent before anything else, so a control plane can decide how to read
    // every frame that follows from the first one it receives. A pod that
    // predates this sends nothing here, which is exactly how it is recognised.
    if params.framed() {
        let lanes = if params.telemetric() {
            ["ctl", lanes::LANE_NAME_TELEMETRY]
        } else if params.conversational() {
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

    if let Some(interval_ms) = params.telemetry {
        return crate::telemetry::relay_telemetry(sink, source, state, id, interval_ms).await;
    }
    if params.conversational() {
        crate::conversation_link::relay_conversation(sink, source, state, id).await
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

    let startup = state.startup_gate.read().await;
    let started: std::io::Result<(
        std::sync::Arc<PtySession>,
        tokio::sync::broadcast::Receiver<Vec<u8>>,
    )> = state
        .registry
        .get_or_start(&id, &state.agent_command(), cols, rows, PLACEHOLDER_KEY)
        .await;
    drop(startup);
    let (session, mut output) = started?;

    let mut exited = session.subscribe_exit();
    session.resize(cols, rows)?;

    // Answers to control frames share the one sink the output pump owns, so
    // the inbound side never needs a second half of the socket.
    let (replies, mut answers) = mpsc::channel::<String>(CONTROL_REPLIES);
    let runtime = id.clone();
    let pump_out = tokio::spawn(async move {
        // A short-lived process may have ended before this task was scheduled.
        // The watch is latched precisely so that case cannot lose the status.
        let mut reported = *exited.borrow_and_update();
        loop {
            if let Some(exit) = reported {
                // Exit is published only after the supervisor stops forwarding
                // pty bytes, so every remaining item was sent before it. Drain
                // those bytes before putting the ending on the control lane.
                loop {
                    match output.try_recv() {
                        Ok(chunk) => {
                            let payload = if framed {
                                lanes::frame(LANE_TERMINAL, &chunk)
                            } else {
                                chunk
                            };
                            if sink.send(Message::Binary(payload.into())).await.is_err() {
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                            tracing::warn!(runtime = %runtime, skipped, "terminal consumer lagged");
                        }
                        Err(
                            tokio::sync::broadcast::error::TryRecvError::Empty
                            | tokio::sync::broadcast::error::TryRecvError::Closed,
                        ) => break,
                    }
                }
                let text = match serde_json::to_string(&OutboundControl::Exit { code: exit.code }) {
                    Ok(text) => text,
                    Err(error) => {
                        tracing::warn!(runtime = %runtime, %error, "process exit could not be rendered");
                        return;
                    }
                };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::NORMAL,
                        reason: "".into(),
                    })))
                    .await;
                break;
            }

            let chunk = tokio::select! {
                chunk = output.recv() => Some(chunk),
                changed = exited.changed() => {
                    match changed {
                        Ok(()) => {
                            reported = *exited.borrow_and_update();
                            continue;
                        }
                        // Waiting failed, so there is no status to report. The
                        // link closes without inventing one.
                        Err(_) => break,
                    }
                }
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
            };
            match chunk.expect("the output branch always carries a receive result") {
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
    let pump_in = tokio::spawn(async move {
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
                        return TerminalInputEnd::PtyWriteFailed;
                    }
                }
                Ok(Message::Text(text)) => {
                    control(&text, &runtime, &state_in, Some(&session_in), &replies).await
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(runtime = %runtime, error = %e, "terminal read failed");
                    break;
                }
            }
        }
        TerminalInputEnd::PeerEnded
    });

    finish_terminal_pumps(pump_out, pump_in).await;
    tracing::info!(runtime = %id, "terminal detached");
    Ok(())
}

/// Finish the terminal halves without losing a process ending to late input.
async fn finish_terminal_pumps(
    mut pump_out: tokio::task::JoinHandle<()>,
    mut pump_in: tokio::task::JoinHandle<TerminalInputEnd>,
) {
    // A dropped connection detaches; it does not end the session. There is no
    // peer left to receive output, so the output task can stop. A failed PTY
    // write instead commonly means the child has just exited while a keystroke
    // was in flight. In that case the output task must drain the latched exit
    // and send its normal ending rather than being aborted by the input race.
    tokio::select! {
        _ = &mut pump_out => pump_in.abort(),
        input_end = &mut pump_in => match input_end {
            Ok(TerminalInputEnd::PtyWriteFailed) => {
                let _ = pump_out.await;
            }
            Ok(TerminalInputEnd::PeerEnded) | Err(_) => pump_out.abort(),
        },
    }
}

/// Act on a control-lane frame.
///
/// Shared by both planes, which is the whole reason the lane exists: a
/// credential reaches a conversation session the same way it reaches a
/// terminal one. Resize and signal need a terminal, and say so rather than
/// pretending when there is none.
pub(crate) async fn control(
    text: &str,
    runtime: &str,
    state: &AppState,
    session: Option<&std::sync::Arc<PtySession>>,
    replies: &mpsc::Sender<String>,
) {
    match serde_json::from_str::<ControlFrame>(text) {
        Ok(ControlFrame::Credential {
            operation_id,
            value,
        }) => {
            // Never logged with its value; the holder reports only that a
            // replacement happened.
            state.credential.set(value).await;
            // Legacy callers sent no operation id and expect no unsolicited
            // result frame. Current callers get an answer only after `set`
            // completed, so the answer is evidence that the new value is live.
            if let Some(operation_id) = operation_id {
                let answer = serde_json::json!({
                    "t": CREDENTIAL_RESULT,
                    "operation_id": operation_id,
                    "installed": true,
                })
                .to_string();
                if replies.send(answer).await.is_err() {
                    tracing::warn!(
                        runtime,
                        "the link closed before credential installation could be acknowledged"
                    );
                }
            }
        }
        Ok(ControlFrame::GithubCapability {
            operation_id,
            envelope,
        }) => {
            let result = state
                .github_capability
                .install(
                    runtime,
                    &envelope,
                    std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                )
                .await;
            let answer = match result {
                Ok(receipt) => serde_json::json!({
                    "t": GITHUB_CAPABILITY_RESULT,
                    "operation_id": operation_id,
                    "accepted": true,
                    "generation": receipt.generation,
                    "fence": receipt.fence,
                    "revoked": receipt.revoked,
                }),
                Err(refusal) => serde_json::json!({
                    "t": GITHUB_CAPABILITY_RESULT,
                    "operation_id": operation_id,
                    "accepted": false,
                    "refusal": refusal.wire_name(),
                }),
            }
            .to_string();
            if replies.send(answer).await.is_err() {
                tracing::warn!(
                    runtime,
                    "the link closed before GitHub capability installation could be acknowledged"
                );
            }
        }
        Ok(ControlFrame::SwitchPlane { command }) => {
            // The program is swapped in place — the pod, its filesystem and
            // its conversation stay. The allowlist is checked in `switch_command`.
            let accepted = state.switch_command(&command);
            // Answered either way. The control plane records an in-place
            // switch only on a positive answer, and rotates when none comes:
            // a pod that predates this frame says nothing at all, and silence
            // is what carries that case to the fallback rather than to a
            // session whose record disagrees with the program it is running.
            let answer = serde_json::json!({
                "t": SWITCH_RESULT,
                "command": command,
                "accepted": accepted,
            })
            .to_string();
            if replies.send(answer).await.is_err() {
                tracing::warn!(
                    runtime,
                    "the link closed before the switch could be answered"
                );
            }
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
            let byte = signal_byte(&sig);
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
