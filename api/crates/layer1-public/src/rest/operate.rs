//! Driving a session: input, interruption, geometry, approvals and usage.
//!
//! The routes that make this interface what the overview calls it — the
//! superset facade, the reference realisation of the core. Until now it served
//! a session's lifecycle and its history and nothing in between: everything an
//! attached terminal could do had no projection outside the WebSocket, and a
//! conversation session had no public entry point at all.
//!
//! ## One route, two planes
//!
//! `input` means different things to the two kinds of session, and that is the
//! projection rather than a compromise. On a terminal it is keystrokes, which
//! is also how that plane answers its kernel's approval prompts and how it
//! steers a turn already running. On a conversation it opens a prompt turn.
//! The session's granted capabilities decide which, so a caller never chooses.
//!
//! Anything a session cannot do is refused by name — the capability class it
//! lacks — rather than with a 404. The route exists; this session simply
//! cannot honour it, and a caller told "not found" goes looking for a spelling
//! mistake.
//!
//! ## Nothing here reads a body into a log
//!
//! Input text, prompts and approval payloads are the owner's own words. The
//! activity record names the operation and the session and stops there.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use layer2_core::{
    ApprovalDecision, CoreError, SessionCapability, SessionId, SessionService, TerminalGeometry,
    Usage,
};
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedCaller;

use super::Problem;

/// What a caller sends to a session.
#[derive(Debug, Deserialize)]
pub(super) struct InputRequest {
    /// Text to deliver.
    #[serde(default)]
    text: Option<String>,
    /// Raw bytes, base64, for a terminal that needs something text cannot say.
    ///
    /// Control characters, escape sequences, and the answers to a kernel's
    /// on-screen prompts. Meaningless on a conversation, and refused there
    /// rather than decoded and sent as a prompt nobody typed.
    #[serde(default)]
    bytes_b64: Option<String>,
    /// Whether to follow the text with a newline.
    ///
    /// Defaults to true, because a terminal client that sends a line means to
    /// submit it; a caller composing a command in pieces says so explicitly.
    #[serde(default = "yes")]
    newline: bool,
}

fn yes() -> bool {
    true
}

/// A geometry change.
#[derive(Debug, Deserialize)]
pub(super) struct ResizeRequest {
    cols: u16,
    rows: u16,
}

/// An owner's answer to something the runtime is blocked on.
#[derive(Debug, Deserialize)]
pub(super) struct ApprovalRequestBody {
    decision: DecisionBody,
}

/// What the owner decided, in the words every interface uses.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DecisionBody {
    /// Let it proceed.
    Allow,
    /// Refuse it; the turn carries on without doing this.
    Deny,
    /// Refuse it and stop the turn.
    Abort,
}

impl From<DecisionBody> for ApprovalDecision {
    fn from(body: DecisionBody) -> Self {
        match body {
            DecisionBody::Allow => Self::Allow,
            DecisionBody::Deny => Self::Deny,
            DecisionBody::Abort => Self::Abort,
        }
    }
}

/// What a session has used.
#[derive(Debug, Serialize)]
pub(super) struct UsageView {
    input_tokens: u64,
    output_tokens: u64,
    context_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
}

impl From<Usage> for UsageView {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            context_used: usage.context_used,
            context_window: usage.context_window,
        }
    }
}

/// Send something to a session, on whichever plane it holds.
pub(super) async fn input(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<InputRequest>,
) -> Result<StatusCode, Problem> {
    let id = SessionId(id);
    let session = service.get_session(&caller, &id).await?;

    if session.capabilities.holds(SessionCapability::Conversation) {
        if body.bytes_b64.is_some() {
            return Err(CoreError::InvalidRequest(
                "a conversation takes text, not terminal bytes".into(),
            )
            .into());
        }
        let text = body
            .text
            .ok_or_else(|| CoreError::InvalidRequest("a prompt needs text to send".to_string()))?;
        service.send_prompt(&caller, &id, text).await?;
        return Ok(StatusCode::ACCEPTED);
    }

    // Refused where somebody is holding the terminal. This caller is not that
    // client — one that were would be writing over its own attachment rather
    // than through this route — so its keystrokes would land inside an escape
    // sequence the holder is drawing.
    if service.terminal_is_held(&caller, &id).await? {
        return Err(CoreError::AlreadyAttached.into());
    }

    // The terminal plane. Bytes win over text when both are sent: a caller
    // that supplied raw bytes meant something text could not express, and
    // guessing which they meant would put characters on somebody's screen.
    let bytes = match (body.bytes_b64, body.text) {
        (Some(encoded), _) => decode(&encoded)?,
        (None, Some(text)) => {
            let mut bytes = text.into_bytes();
            if body.newline {
                bytes.push(b'\n');
            }
            bytes
        }
        (None, None) => {
            return Err(
                CoreError::InvalidRequest("input needs text or bytes to send".into()).into(),
            )
        }
    };
    service.send_input(&caller, &id, bytes).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Stop what the session is doing, without ending it.
pub(super) async fn interrupt(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    let id = SessionId(id);
    let session = service.get_session(&caller, &id).await?;
    match session.capabilities.holds(SessionCapability::Conversation) {
        true => service.interrupt_turn(&caller, &id).await?,
        // Delivered even while a client is attached: a signal is not a
        // keystroke, and stopping a runaway command is exactly what an owner
        // may need to do from elsewhere while a screen is held open.
        false => service.send_signal(&caller, &id, "INT".to_string()).await?,
    }
    Ok(StatusCode::ACCEPTED)
}

/// Tell the runtime its terminal changed size.
pub(super) async fn resize(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<ResizeRequest>,
) -> Result<StatusCode, Problem> {
    let id = SessionId(id);
    let geometry = TerminalGeometry::new(body.cols, body.rows).map_err(CoreError::from)?;
    // The attached client declared the geometry it is drawing for, and
    // resizing underneath it reflows a screen it is halfway through painting.
    if service.terminal_is_held(&caller, &id).await? {
        return Err(CoreError::AlreadyAttached.into());
    }
    service.resize_terminal(&caller, &id, geometry).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Answer something the runtime is blocked on.
pub(super) async fn resolve_approval(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, request_id)): Path<(String, String)>,
    Json(body): Json<ApprovalRequestBody>,
) -> Result<StatusCode, Problem> {
    service
        .resolve_approval(&caller, &SessionId(id), request_id, body.decision.into())
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// What this session has used.
pub(super) async fn usage(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<UsageView>, Problem> {
    let usage = service.get_usage(&caller, &SessionId(id)).await?;
    Ok(Json(usage.into()))
}

/// Redirect a turn already in progress.
///
/// Its own route because it is its own operation: a caller that means "add to
/// what you are already doing" is saying something different from "here is a
/// new turn", and a kernel that cannot tell them apart must say so rather than
/// silently treat one as the other.
pub(super) async fn steer(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<InputRequest>,
) -> Result<StatusCode, Problem> {
    let id = SessionId(id);
    let session = service.get_session(&caller, &id).await?;
    // Declared, not discovered. A session whose kernel does not steer refuses
    // here rather than accepting the request and delivering it as an ordinary
    // turn, which would be a different thing done under the caller's name.
    session.capabilities.require(SessionCapability::Steer)?;
    let text = body
        .text
        .ok_or_else(|| CoreError::InvalidRequest("steering needs text to send".to_string()))?;
    service.send_prompt(&caller, &id, text).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Decode a base64 body, saying what is wrong without quoting it.
fn decode(encoded: &str) -> Result<Vec<u8>, Problem> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| CoreError::InvalidRequest("bytes_b64 is not valid base64".into()).into())
}

#[cfg(test)]
#[path = "operate_tests.rs"]
mod operate_tests;
