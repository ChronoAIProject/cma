//! Stable REST projections for agent tools.
use super::problems::{problem, protocol_headers};
use crate::{
    auth::AgentCaller,
    delivery::intents::{IntentReceipt, TypingReceipt},
    tools::{ContextResponse, ToolsState},
};
use axum::{
    extract::{Path, State},
    http::{header::CONTENT_TYPE, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::EventRef,
};
use serde::Serialize;
use tower_http::limit::RequestBodyLimitLayer;

pub fn router(state: ToolsState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .merge(super::reply_targets::routes())
            .route("/v1/events/{event_ref}/replies", post(reply_to_event))
            .route("/v1/messages/{message_ref}/update", post(update_message))
            .route("/v1/events/{event_ref}/typing", post(send_typing))
            .route(
                "/v1/events/{event_ref}/context",
                get(read_event_context).post(verify_event_context),
            )
            .with_state(state)
            .layer(RequestBodyLimitLayer::new(131072))
            .layer(axum::middleware::from_fn(protocol_headers)),
    )
}
#[utoipa::path(post,path="/v1/events/{event_ref}/replies",operation_id="replyToEvent",tag="tools",
    security(("agentBearer"=[])),params(("event_ref"=String,Path)),request_body=OutboundMessage,
    responses((status=200,body=IntentReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn reply_to_event(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    body: Result<Json<OutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let message = match json(body) {
        Ok(value) => value,
        Err(error) => return result::<IntentReceipt>(Err(error)),
    };
    let reference = match event_ref(&reference) {
        Ok(value) => value,
        Err(error) => return result::<IntentReceipt>(Err(error)),
    };
    result(state.reply(caller, &reference, message).await)
}
#[utoipa::path(post,path="/v1/messages/{message_ref}/update",operation_id="updateMessage",tag="tools",
    security(("agentBearer"=[])),params(("message_ref"=String,Path)),request_body=OutboundMessage,
    responses((status=200,body=IntentReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn update_message(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    body: Result<Json<OutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let message = match json(body) {
        Ok(value) => value,
        Err(error) => return result::<IntentReceipt>(Err(error)),
    };
    let reference = match MessageRef::new(reference) {
        Ok(value) => value,
        Err(_) => {
            return result::<IntentReceipt>(Err(Problem::new(
                ProblemCode::EventRefInvalid,
                400,
                "message_ref_invalid",
            )))
        }
    };
    result(state.update(caller, &reference, message).await)
}
#[utoipa::path(post,path="/v1/events/{event_ref}/typing",operation_id="sendTyping",tag="tools",
    security(("agentBearer"=[])),params(("event_ref"=String,Path)),
    responses((status=200,body=TypingReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn send_typing(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    headers: HeaderMap,
) -> Response {
    if headers.get_all(CONTENT_TYPE).iter().count() != 1
        || !headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return result::<TypingReceipt>(Err(Problem::new(
            ProblemCode::UnsupportedMediaType,
            415,
            "application/json is required",
        )));
    }
    let reference = match event_ref(&reference) {
        Ok(value) => value,
        Err(error) => return result::<TypingReceipt>(Err(error)),
    };
    result(state.typing(caller, &reference).await)
}
#[utoipa::path(get,path="/v1/events/{event_ref}/context",operation_id="readEventContext",tag="tools",
    security(("agentBearer"=[])),params(("event_ref"=String,Path)),
    responses((status=200,body=ContextResponse),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read_event_context(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
) -> Response {
    let reference = match event_ref(&reference) {
        Ok(value) => value,
        Err(error) => return result::<ContextResponse>(Err(error)),
    };
    result(state.context(caller, &reference).await)
}
#[utoipa::path(post,path="/v1/events/{event_ref}/context",operation_id="verifyEventContext",tag="provider integration",
    security(("agentBearer"=[])),params(("event_ref"=String,Path)),request_body=cmaeg_core::activity::Activity,
    responses((status=200,body=ContextResponse),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn verify_event_context(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    body: Result<Json<cmaeg_core::activity::Activity>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let activity = match json(body) {
        Ok(value) => value,
        Err(error) => return result::<ContextResponse>(Err(error)),
    };
    let reference = match event_ref(&reference) {
        Ok(value) => value,
        Err(error) => return result::<ContextResponse>(Err(error)),
    };
    result(state.verify_context(caller, &reference, activity).await)
}
pub(super) fn json<T>(
    body: Result<Json<T>, axum::extract::rejection::JsonRejection>,
) -> Result<T, Problem> {
    body.map(|Json(value)| value).map_err(|error| {
        let (status, code, detail) = match error {
            axum::extract::rejection::JsonRejection::MissingJsonContentType(_) => (
                415,
                ProblemCode::UnsupportedMediaType,
                "application/json is required",
            ),
            axum::extract::rejection::JsonRejection::BytesRejection(_)
                if error.status() == StatusCode::PAYLOAD_TOO_LARGE =>
            {
                (413, ProblemCode::PayloadTooLarge, "tool body exceeds limit")
            }
            _ => (
                400,
                ProblemCode::InvalidRequest,
                "invalid tool request body",
            ),
        };
        Problem::new(code, status, detail)
    })
}
pub(crate) fn event_ref(raw: &str) -> Result<EventRef, Problem> {
    raw.parse()
        .map_err(|_| Problem::new(ProblemCode::EventRefInvalid, 400, "event_ref_invalid"))
}
pub(crate) fn result<T: Serialize>(value: Result<T, Problem>) -> Response {
    match value {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            error.code,
            error.detail.unwrap_or_default(),
        ),
    }
}
