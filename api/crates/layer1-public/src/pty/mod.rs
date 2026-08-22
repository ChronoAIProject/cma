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
        ws::{CloseFrame, Message, WebSocket},
        FromRef, Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use layer2_core::{
    AttachmentClaim, Caller, CoreError, SessionId, SessionService, TerminalGeometry,
};
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedCaller;
use crate::tickets::TerminalTickets;
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

/// Close code for a client whose attachment was taken over.
///
/// In the private range, because no registered code says this. 4000 is the
/// first application-defined code and nothing else in this interface uses one.
const SUPERSEDED_CLOSE_CODE: u16 = 4000;

/// Close code for a terminal whose session moved onto the conversation plane.
///
/// Its own code rather than the one above, because the two ask different
/// things of whoever is looking: a superseded client goes to the tab that took
/// the terminal, and this one has no terminal to go to at all — the session is
/// still there and is now a conversation.
const PLANE_CHANGED_CLOSE_CODE: u16 = 4001;

/// Close code for a terminal whose session is being carried onto a newer runtime.
///
/// Distinguished for the same reason: the only thing this asks of anyone is to
/// come back in a moment.
const RUNTIME_REPLACED_CLOSE_CODE: u16 = 4002;

/// Why this terminal ended, in a code and a sentence.
///
/// Every path through here says something. A terminal that closes with nothing
/// is one whose owner reloads and takes the attachment back, which is the
/// fight the reasons exist to prevent.
fn ending(detached: Option<layer2_core::Detached>) -> (u16, &'static str) {
    match detached {
        Some(layer2_core::Detached::PlaneChanged) => (
            PLANE_CHANGED_CLOSE_CODE,
            "this session moved to the conversation plane and no longer has a terminal",
        ),
        Some(layer2_core::Detached::RuntimeReplaced) => (
            RUNTIME_REPLACED_CLOSE_CODE,
            "this session is being carried onto an updated runtime; attach again in a moment",
        ),
        // Including the channel closing with nothing said: the attachment is
        // gone either way, and the reading that costs least is the one that
        // does not send somebody looking for a runtime that is fine.
        Some(layer2_core::Detached::Superseded) | None => (
            SUPERSEDED_CLOSE_CODE,
            "the owner attached this terminal from somewhere else",
        ),
    }
}

/// How often the server proves the connection is still there.
///
/// Under the shortest idle timeout these connections pass through, so a silent
/// terminal is never mistaken for a dead one. Cheap: an empty frame.
const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

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

/// What the terminal route is served from.
#[derive(Clone)]
pub struct TerminalState {
    /// Sessions and the runtimes serving them.
    pub sessions: Arc<SessionService>,
    /// Tickets minted by the REST surface, redeemed here.
    pub tickets: Arc<TerminalTickets>,
}

impl FromRef<TerminalState> for Arc<SessionService> {
    fn from_ref(state: &TerminalState) -> Self {
        Arc::clone(&state.sessions)
    }
}

impl FromRef<TerminalState> for Arc<TerminalTickets> {
    fn from_ref(state: &TerminalState) -> Self {
        Arc::clone(&state.tickets)
    }
}

/// Routes this interface contributes.
pub fn router(state: TerminalState) -> Router {
    Router::new()
        .route("/api/v1/sessions/{id}/terminal", get(attach))
        .with_state(state)
}

/// How an upgrade may present itself.
///
/// A ticket is accepted here and nowhere else, and its value is never read
/// back out of this struct for anything but redemption. The activity
/// classifier's query allowlist does not name it, so it cannot reach a record;
/// nothing below logs it either, and the temptation to log one while chasing a
/// failing attach should be answered with the session and the subject instead.
#[derive(Debug, Default, Deserialize)]
struct AttachParams {
    #[serde(default)]
    ticket: Option<String>,
}

/// Decide who is attaching, and on what terms.
///
/// A ticket wins where one is presented, because a client that went to the
/// trouble of getting one meant to use it — and a ticket carries a claim that
/// an ambient cookie does not. Falling back to the cookie after a refused
/// ticket would let a stale or spent ticket succeed anyway on a same-origin
/// page, which is the opposite of what single use is for.
async fn admit(
    tickets: &TerminalTickets,
    id: &SessionId,
    ticket: Option<String>,
    caller: Option<AuthenticatedCaller>,
) -> Option<(Caller, AttachmentClaim)> {
    match ticket {
        Some(ticket) => tickets
            .redeem(&ticket, id)
            .await
            .map(|r| (r.caller, r.claim)),
        // The cookie path, unchanged: middleware verified the credential and
        // the caller attaches as themselves, exclusively.
        None => caller.map(|AuthenticatedCaller(c)| (c, AttachmentClaim::Exclusive)),
    }
}

/// Attach a session's terminal.
///
/// Two ways in, because two kinds of client exist. A same-origin caller —
/// our own dashboard — arrives with a cookie the browser sends itself, and the
/// ordinary middleware has already established who they are. A cross-origin
/// caller holding a bearer token cannot put it on a WebSocket upgrade at all,
/// so it exchanges it for a ticket first and presents that.
async fn attach(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    Query(params): Query<AttachParams>,
    State(service): State<Arc<SessionService>>,
    State(tickets): State<Arc<TerminalTickets>>,
    caller: Option<AuthenticatedCaller>,
) -> impl IntoResponse {
    let id = SessionId(id);
    let Some((caller, claim)) = admit(&tickets, &id, params.ticket, caller).await else {
        // Refused before the upgrade, so the client gets an HTTP status it can
        // act on rather than a socket that opens and immediately closes.
        return crate::auth::NotAuthenticated.into_response();
    };
    ws.on_upgrade(move |socket| serve(socket, service, id, caller, claim))
        .into_response()
}

/// Translate a core failure into this interface's error vocabulary.
///
/// The interface only renames; it never decides. In particular it does not
/// compute whether a caller is allowed to know a session exists — the core
/// already collapsed that distinction.
fn error_code(err: &CoreError) -> &'static str {
    match err {
        CoreError::SessionNotFound | CoreError::Forbidden => "session_not_found",
        // This interface drives an existing session's terminal and never
        // authors an environment profile, so a profile it cannot see is
        // reported the same way a session it cannot see is.
        CoreError::EnvironmentProfileNotFound => "session_not_found",
        CoreError::InvalidRequest(_) => "invalid_request",
        CoreError::AlreadyAttached => "already_attached",
        CoreError::SessionTerminated => "session_terminated",
        CoreError::InvalidGeometry(_) => "invalid_geometry",
        CoreError::RuntimeUnavailable(_) => "runtime_unavailable",
        CoreError::RuntimeRefused(_) => "runtime_refused",
        CoreError::InvalidState => "invalid_state",
        CoreError::CapabilityUnsupported(_) => "capability_unsupported",
        // This interface attaches to a session that already exists, so a
        // service refusal cannot arise here — a session that reached this far
        // was already provisioned past it. Named rather than folded into a
        // catch-all so that adding a variant to the core is a compile error
        // here rather than a frame that quietly says the wrong thing.
        CoreError::ServiceNotGranted(_) => "service_not_granted",
        CoreError::ServiceCatalogUnavailable(_) => "service_catalogue_unavailable",
        // The platform's own settings are not something this interface reads
        // or writes: it drives an existing session's terminal. Named for the
        // reason above — so a variant added to the core is a compile error
        // here rather than a frame that quietly says the wrong thing.
        CoreError::SettingsConflict(_) => "settings_conflict",
        CoreError::SettingsUnavailable(_) => "settings_unavailable",
    }
}

async fn serve(
    socket: WebSocket,
    service: Arc<SessionService>,
    id: SessionId,
    caller: Caller,
    claim: AttachmentClaim,
) {
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

    let attachment = match service.attach_terminal(&caller, &id, geometry, claim).await {
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
    let mut superseded = attachment.superseded.resubscribe();
    let mut pump_out = tokio::spawn(async move {
        // A terminal is mostly silent — someone reading output produces no
        // traffic at all — and an idle connection is exactly what proxies and
        // load balancers reclaim. Without this the session dies mid-thought
        // for no reason the user can see.
        //
        // A protocol-level ping rather than an application one, because
        // browsers answer it themselves: the reply proves the path is still
        // open in both directions without the page having to do anything.
        let mut heartbeat = tokio::time::interval(KEEPALIVE_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first tick is immediate and would send a ping before anything
        // has had a chance to go idle.
        heartbeat.tick().await;
        loop {
            tokio::select! {
                received = output.recv() => match received {
                    Ok(chunk) => {
                        if sink.send(Message::Binary(chunk.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "terminal client lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = heartbeat.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                // This attachment has been taken away. Said rather than shown,
                // and said accurately: a terminal that simply stops updating is
                // indistinguishable from a broken one, and one that names the
                // wrong reason sends its owner somewhere there is nothing to
                // find.
                taken = superseded.recv() => {
                    let (code, reason) = ending(taken.ok());
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code,
                            reason: reason.into(),
                        })))
                        .await;
                    break;
                }
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
                    // Answered by the heartbeat above rather than here: this
                    // half of the socket cannot write. A client ping still
                    // serves its purpose, since the traffic itself is what
                    // keeps an intermediary from reclaiming the connection.
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
#[path = "tests.rs"]
mod tests;
