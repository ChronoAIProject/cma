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
    response::IntoResponse,
    Json,
};
use layer2_core::{
    format_usd, ApprovalDecision, CoreError, OperationStore, ResourceReading, ResourceStatus,
    SessionCapability, SessionId, SessionService, Spend, TerminalGeometry, Usage, CURRENCY,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthenticatedCaller;

use super::{idempotency, Problem, RequestFailure};

/// What a caller sends to a session.
#[derive(Debug, Deserialize, ToSchema)]
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
    #[schema(default = true)]
    newline: bool,
}

fn yes() -> bool {
    true
}

/// A geometry change.
#[derive(Debug, Deserialize, ToSchema)]
pub(super) struct ResizeRequest {
    cols: u16,
    rows: u16,
}

/// An owner's answer to something the runtime is blocked on.
#[derive(Debug, Deserialize, ToSchema)]
pub(super) struct ApprovalRequestBody {
    decision: DecisionBody,
}

/// What the owner decided, in the words every interface uses.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
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

/// One resource reading in REST's snake-case vocabulary.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct ResourceReadingView {
    cpu_millis_used: Option<u32>,
    cpu_millis_limit: Option<u32>,
    memory_bytes_used: Option<u64>,
    memory_bytes_limit: Option<u64>,
    memory_bytes_peak: Option<u64>,
    stall_permille: Option<u16>,
    ceiling_hits: Option<u64>,
    allocation_failures: Option<u64>,
    kills: Option<u64>,
    received_bytes: Option<u64>,
    sent_bytes: Option<u64>,
    received_bytes_per_second: Option<u64>,
    sent_bytes_per_second: Option<u64>,
    interval_ms: Option<u64>,
}

impl From<ResourceReading> for ResourceReadingView {
    fn from(reading: ResourceReading) -> Self {
        let ResourceReading {
            cpu_millis_used,
            cpu_millis_limit,
            memory_bytes_used,
            memory_bytes_limit,
            memory_bytes_peak,
            stall_permille,
            ceiling_hits,
            allocation_failures,
            kills,
            received_bytes,
            sent_bytes,
            received_bytes_per_second,
            sent_bytes_per_second,
            interval_ms,
        } = reading;
        Self {
            cpu_millis_used,
            cpu_millis_limit,
            memory_bytes_used,
            memory_bytes_limit,
            memory_bytes_peak,
            stall_permille,
            ceiling_hits,
            allocation_failures,
            kills,
            received_bytes,
            sent_bytes,
            received_bytes_per_second,
            sent_bytes_per_second,
            interval_ms,
        }
    }
}

/// What is known about a session's resource use at this moment.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct ResourceStatusView {
    reading: Option<ResourceReadingView>,
    observed_at: Option<String>,
    reason: Option<&'static str>,
}

impl From<ResourceStatus> for ResourceStatusView {
    fn from(status: ResourceStatus) -> Self {
        match status {
            ResourceStatus::Held(held) => Self {
                reading: Some(held.reading.into()),
                observed_at: Some(held.observed_at),
                reason: None,
            },
            ResourceStatus::Awaiting => Self {
                reading: None,
                observed_at: None,
                reason: Some("no_reading_yet"),
            },
            ResourceStatus::Unreported => Self {
                reading: None,
                observed_at: None,
                reason: Some("runtime_does_not_report"),
            },
        }
    }
}

/// Reserved-capacity spend derived from one session's durable history.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct SpendView {
    priced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_card: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<&'static str>,
    /// Decimal strings keep monetary integers out of JavaScript numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    micros_per_hour: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accrued_micros: Option<String>,
    /// Rendered by the same integer formatter used for the quoted rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    billable_seconds: Option<u64>,
    accruing: bool,
    unbounded_intervals: u32,
}

impl From<Spend> for SpendView {
    fn from(spend: Spend) -> Self {
        Self {
            currency: spend.priced.then_some(CURRENCY),
            rate_card: spend.rate_card.map(|id| id.0),
            micros_per_hour: spend.micros_per_hour.map(|value| value.to_string()),
            accrued_micros: spend.accrued_micros.map(|value| value.to_string()),
            display: spend.accrued_micros.map(format_usd),
            billable_seconds: spend.billable_seconds,
            priced: spend.priced,
            accruing: spend.accruing,
            unbounded_intervals: spend.unbounded_intervals,
        }
    }
}

/// Send something to a session, on whichever plane it holds.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/input",
    tag = "operations",
    params(
        ("id" = String, Path),
        ("Idempotency-Key" = Option<String>, Header)
    ),
    request_body = InputRequest,
    responses(
        (status = 202, description = "Input accepted"),
        (status = 400, description = "Malformed operation key"),
        (status = 409, description = "Operation with this key is still running"),
        (status = 422, description = "Operation key names a different request"),
        (status = 503, description = "Operation store unavailable")
    )
)]
pub(super) async fn input(
    State(service): State<Arc<SessionService>>,
    State(operations): State<Arc<dyn OperationStore>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: idempotency::IdempotencyKey,
    Path(id): Path<String>,
    body: Result<idempotency::RequestBody<InputRequest>, super::BodyProblem>,
) -> axum::response::Response {
    let (body, fingerprint) = match body {
        Ok(body) => body.into_parts(),
        Err(error) => return RequestFailure::from(error).into_response(),
    };
    let id = SessionId(id);
    let guard = match idempotency::Idempotency::begin(
        key,
        operations,
        &caller,
        format!("session-input:{}", id.as_str()),
        fingerprint,
    )
    .await
    {
        Ok(idempotency::Idempotency::Replay(response)) => return response,
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let body = match body {
        Ok(body) => body,
        Err(error) => return guard.finish(Err(RequestFailure::from(error))).await,
    };
    let result = input_once(&service, &caller, &id, body)
        .await
        .map(IntoResponse::into_response);
    guard.finish(result).await
}

async fn input_once(
    service: &Arc<SessionService>,
    caller: &layer2_core::Caller,
    id: &SessionId,
    body: InputRequest,
) -> Result<StatusCode, RequestFailure> {
    let session = service.get_session(caller, id).await?;

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
        service.send_prompt(caller, id, text).await?;
        return Ok(StatusCode::ACCEPTED);
    }

    // Refused where somebody is holding the terminal. This caller is not that
    // client — one that were would be writing over its own attachment rather
    // than through this route — so its keystrokes would land inside an escape
    // sequence the holder is drawing.
    if service.terminal_is_held(caller, id).await? {
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
    service.send_input(caller, id, bytes).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Stop what the session is doing, without ending it.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/interrupt",
    tag = "operations",
    params(("id" = String, Path)),
    responses((status = 202, description = "Interrupt accepted"))
)]
pub(super) async fn interrupt(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    let id = SessionId(id);
    let session = service.get_session(&caller, &id).await?;
    match session.capabilities.holds(SessionCapability::Conversation) {
        true => service.interrupt_turn(&caller, &id).await?,
        // Delivered even while a client is attached: this injects only the
        // bounded INT control byte, and stopping a runaway command is exactly
        // what an owner may need to do while a screen is held elsewhere.
        false => service.send_signal(&caller, &id, "INT".to_string()).await?,
    }
    Ok(StatusCode::ACCEPTED)
}

/// Tell the runtime its terminal changed size.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/resize",
    tag = "terminal",
    params(("id" = String, Path)),
    request_body = ResizeRequest,
    responses((status = 204, description = "Terminal resized"))
)]
pub(super) async fn resize(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<ResizeRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, RequestFailure> {
    let Json(body) = body?;
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
#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/approvals/{request_id}",
    tag = "operations",
    params(("id" = String, Path), ("request_id" = String, Path)),
    request_body = ApprovalRequestBody,
    responses((status = 202, description = "Approval decision accepted"))
)]
pub(super) async fn resolve_approval(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, request_id)): Path<(String, String)>,
    body: Result<Json<ApprovalRequestBody>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, RequestFailure> {
    let Json(body) = body?;
    service
        .resolve_approval(&caller, &SessionId(id), request_id, body.decision.into())
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// What this session has used.
#[utoipa::path(
    get,
    path = "/api/v1/sessions/{id}/usage",
    tag = "usage",
    params(("id" = String, Path)),
    responses((status = 200, description = "Token usage", body = UsageView))
)]
pub(super) async fn usage(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<UsageView>, Problem> {
    let usage = service.get_usage(&caller, &SessionId(id)).await?;
    Ok(Json(usage.into()))
}

/// What this session is using right now, or why no reading exists.
#[utoipa::path(
    get,
    path = "/api/v1/sessions/{id}/resources",
    tag = "usage",
    params(("id" = String, Path)),
    responses((status = 200, description = "Latest sandbox resource reading", body = ResourceStatusView))
)]
pub(super) async fn resources(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<ResourceStatusView>, Problem> {
    let status = service.last_reading(&caller, &SessionId(id)).await?;
    Ok(Json(status.into()))
}

/// What this session has accrued for reserved capacity.
#[utoipa::path(
    get,
    path = "/api/v1/sessions/{id}/spend",
    tag = "pricing",
    params(("id" = String, Path)),
    responses((status = 200, description = "Accrued reserved-capacity spend", body = SpendView))
)]
pub(super) async fn spend(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<SpendView>, Problem> {
    let spend = service.get_spend(&caller, &SessionId(id)).await?;
    Ok(Json(spend.into()))
}

/// Redirect a turn already in progress.
///
/// Its own route because it is its own operation: a caller that means "add to
/// what you are already doing" is saying something different from "here is a
/// new turn", and a kernel that cannot tell them apart must say so rather than
/// silently treat one as the other.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/steer",
    tag = "operations",
    params(("id" = String, Path)),
    request_body = InputRequest,
    responses((status = 202, description = "Steering input accepted"))
)]
pub(super) async fn steer(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<InputRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, RequestFailure> {
    let Json(body) = body?;
    let id = SessionId(id);
    let session = service.session_for_interaction(&caller, &id).await?;
    // Declared, not discovered. A session whose kernel does not steer refuses
    // here rather than accepting the request and delivering it as an ordinary
    // turn, which would be a different thing done under the caller's name.
    // Lifecycle belongs to Layer 2 and precedes this operation-specific check,
    // so capabilities retained from the outgoing plane cannot mask rotation.
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
