//! PTY interface — for CLI-like streaming clients.
//!
//! Relays a session's terminal byte for byte, so a browser attaching to it sees
//! exactly what a developer running the kernel in a local terminal would see,
//! full-screen interface included.
//!
//! Framing is bimodal with no escaping: binary frames are terminal bytes, text
//! frames are JSON control messages. Dispatch is on the frame opcode, never on
//! inspecting the payload.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use layer2_core::{Caller, CoreError, SessionId, SessionService, TerminalGeometry};
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedCaller;
use crate::{InterfaceProfile, Operation};

/// Conformance classes this interface satisfies.
const CONFORMS_TO: &[&str] = &[
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/pty-core",
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/terminal",
];

/// Scrollback the core retains for a snapshot.
const SCROLLBACK_LINES: u32 = 2000;

/// Largest inbound frame accepted, in bytes.
const MAX_FRAME_BYTES: u32 = 262_144;

/// The interface profile.
pub struct PtyInterface;

impl InterfaceProfile for PtyInterface {
    fn interface_id(&self) -> &'static str {
        "pty"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::ResizeTerminal,
            Operation::ScreenSnapshot,
        ]
    }
}

/// Inbound control vocabulary.
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ClientControl {
    Resize { cols: u16, rows: u16 },
    Signal { sig: String },
    Ping,
}

/// Declared limits, carried on the ready frame.
#[derive(Debug, Serialize)]
struct Limits {
    #[serde(rename = "scrollbackLines")]
    scrollback_lines: u32,
    #[serde(rename = "maxFrameBytes")]
    max_frame_bytes: u32,
}

/// Outbound control vocabulary.
#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ServerControl {
    Ready {
        cols: u16,
        rows: u16,
        snapshot: bool,
        #[serde(rename = "conformsTo")]
        conforms_to: &'static [&'static str],
        limits: Limits,
    },
    Error {
        code: &'static str,
        detail: String,
    },
    Pong,
}

impl ServerControl {
    fn to_message(&self) -> Message {
        Message::Text(
            serde_json::to_string(self)
                .unwrap_or_else(|_| r#"{"t":"error","code":"internal","detail":""}"#.to_string())
                .into(),
        )
    }
}

/// Routes this interface contributes.
pub fn router(service: Arc<SessionService>) -> Router {
    Router::new()
        .route("/api/v1/sessions/{id}/terminal", get(attach))
        .with_state(service)
}

async fn attach(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| serve(socket, service, SessionId(id), caller))
}

/// Translate a core failure into this interface's error vocabulary.
///
/// The interface only renames; it never decides. In particular it does not
/// compute whether a caller is allowed to know a session exists — the core
/// already collapsed that distinction.
fn error_code(err: &CoreError) -> &'static str {
    match err {
        CoreError::SessionNotFound | CoreError::Forbidden => "session_not_found",
        CoreError::AlreadyAttached => "already_attached",
        CoreError::SessionTerminated => "session_terminated",
        CoreError::InvalidGeometry(_) => "invalid_geometry",
        CoreError::RuntimeUnavailable(_) => "runtime_unavailable",
        CoreError::InvalidState => "invalid_state",
    }
}

async fn serve(socket: WebSocket, service: Arc<SessionService>, id: SessionId, caller: Caller) {
    let (mut sink, mut source) = socket.split();

    // The client must declare its geometry before any terminal byte is sent.
    // A full-screen application painted at the wrong size produces corruption
    // that does not self-heal until the next full repaint, so the first paint
    // has to be at the client's real size.
    let geometry = loop {
        match source.next().await {
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<ClientControl>(&text) {
                    Ok(ClientControl::Resize { cols, rows }) => {
                        match TerminalGeometry::new(cols, rows) {
                            Ok(g) => break g,
                            Err(e) => {
                                let _ = sink
                                    .send(
                                        ServerControl::Error {
                                            code: "invalid_geometry",
                                            detail: e.to_string(),
                                        }
                                        .to_message(),
                                    )
                                    .await;
                                // A bad geometry is ignored, not fatal: the
                                // client is mid-layout and will send another.
                            }
                        }
                    }
                    Ok(ClientControl::Ping) => {
                        let _ = sink.send(ServerControl::Pong.to_message()).await;
                    }
                    Ok(ClientControl::Signal { .. }) => {
                        let _ = sink
                            .send(
                                ServerControl::Error {
                                    code: "malformed_control",
                                    detail: "resize must precede any other frame".into(),
                                }
                                .to_message(),
                            )
                            .await;
                    }
                    Err(e) => {
                        let _ = sink
                            .send(
                                ServerControl::Error {
                                    code: "malformed_control",
                                    detail: e.to_string(),
                                }
                                .to_message(),
                            )
                            .await;
                    }
                }
            }
            Some(Ok(Message::Binary(_))) => {
                let _ = sink
                    .send(
                        ServerControl::Error {
                            code: "malformed_control",
                            detail: "resize must precede the first data frame".into(),
                        }
                        .to_message(),
                    )
                    .await;
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return,
        }
    };

    let attachment = match service.attach_terminal(&caller, &id, geometry).await {
        Ok(a) => a,
        Err(e) => {
            let code = error_code(&e);
            tracing::info!(session_id = %id, %code, "attach refused");
            let _ = sink
                .send(
                    ServerControl::Error {
                        code,
                        detail: e.to_string(),
                    }
                    .to_message(),
                )
                .await;
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
    };

    let _ = sink
        .send(
            ServerControl::Ready {
                cols: attachment.geometry.columns(),
                rows: attachment.geometry.rows(),
                snapshot: true,
                conforms_to: CONFORMS_TO,
                limits: Limits {
                    scrollback_lines: SCROLLBACK_LINES,
                    max_frame_bytes: MAX_FRAME_BYTES,
                },
            }
            .to_message(),
        )
        .await;

    // The snapshot repaints the current screen. It is sent as binary because it
    // is terminal bytes, not structure.
    if !attachment.repaint.is_empty() {
        let _ = sink
            .send(Message::Binary(attachment.repaint.clone().into()))
            .await;
    }

    let mut output = attachment.output.resubscribe();
    let mut pump_out = tokio::spawn(async move {
        loop {
            match output.recv().await {
                Ok(chunk) => {
                    if sink.send(Message::Binary(chunk.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "terminal client lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let svc = Arc::clone(&service);
    let sid = id.clone();
    let caller_in = caller.clone();
    let mut pump_in = tokio::spawn(async move {
        while let Some(frame) = source.next().await {
            match frame {
                Ok(Message::Binary(bytes)) => {
                    if bytes.len() as u32 > MAX_FRAME_BYTES {
                        tracing::warn!(session_id = %sid, len = bytes.len(), "oversized frame");
                        break;
                    }
                    if svc
                        .send_input(&caller_in, &sid, bytes.to_vec())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Text(text)) => match serde_json::from_str::<ClientControl>(&text) {
                    Ok(ClientControl::Resize { cols, rows }) => {
                        match TerminalGeometry::new(cols, rows) {
                            Ok(g) => {
                                let _ = svc.resize_terminal(&caller_in, &sid, g).await;
                            }
                            Err(e) => tracing::debug!(error = %e, "zero geometry ignored"),
                        }
                    }
                    Ok(ClientControl::Signal { sig }) => {
                        let _ = svc.send_signal(&caller_in, &sid, sig).await;
                    }
                    Ok(ClientControl::Ping) => {}
                    Err(e) => tracing::debug!(error = %e, "malformed control ignored"),
                },
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = &mut pump_out => pump_in.abort(),
        _ = &mut pump_in => pump_out.abort(),
    }
    // Dropping the attachment releases the exclusive lease. The session keeps
    // running: a dropped connection detaches, it does not terminate.
    drop(attachment);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_matches_what_the_protocol_can_express() {
        let ops = PtyInterface.operations();
        assert!(ops.contains(&Operation::SendInput));
        assert!(ops.contains(&Operation::ResizeTerminal));
        assert!(ops.contains(&Operation::ScreenSnapshot));
        // Sessions are created over a request/response interface first; this
        // one only ever attaches to an existing session.
        assert!(!ops.contains(&Operation::CreateSession));
        assert!(!ops.contains(&Operation::GetUsage));
        assert!(!ops.contains(&Operation::Steer));
    }

    #[test]
    fn unauthorized_and_missing_collapse_to_one_code() {
        assert_eq!(error_code(&CoreError::SessionNotFound), "session_not_found");
        assert_eq!(error_code(&CoreError::Forbidden), "session_not_found");
    }

    #[test]
    fn each_remaining_failure_has_its_own_code() {
        assert_eq!(error_code(&CoreError::AlreadyAttached), "already_attached");
        assert_eq!(
            error_code(&CoreError::SessionTerminated),
            "session_terminated"
        );
        assert_eq!(
            error_code(&CoreError::RuntimeUnavailable("x".into())),
            "runtime_unavailable"
        );
    }

    #[test]
    fn ready_frame_uses_the_documented_field_names() {
        let msg = ServerControl::Ready {
            cols: 120,
            rows: 40,
            snapshot: true,
            conforms_to: CONFORMS_TO,
            limits: Limits {
                scrollback_lines: SCROLLBACK_LINES,
                max_frame_bytes: MAX_FRAME_BYTES,
            },
        };
        let json = serde_json::to_value(&msg).expect("serializes");
        assert_eq!(json["t"], "ready");
        assert_eq!(json["cols"], 120);
        assert_eq!(json["conformsTo"][0], CONFORMS_TO[0]);
        assert_eq!(json["limits"]["scrollbackLines"], 2000);
        assert_eq!(json["limits"]["maxFrameBytes"], 262_144);
    }

    #[test]
    fn control_frames_parse_from_their_tag() {
        let r: ClientControl =
            serde_json::from_str(r#"{"t":"resize","cols":80,"rows":24}"#).expect("resize parses");
        assert!(matches!(r, ClientControl::Resize { cols: 80, rows: 24 }));
        let s: ClientControl =
            serde_json::from_str(r#"{"t":"signal","sig":"INT"}"#).expect("signal parses");
        assert!(matches!(s, ClientControl::Signal { .. }));
        assert!(serde_json::from_str::<ClientControl>(r#"{"t":"nope"}"#).is_err());
    }
}
