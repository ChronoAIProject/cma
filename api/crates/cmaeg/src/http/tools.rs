//! Stable REST projections for agent tools.
use super::problems::{problem, protocol_headers};
use crate::channels::write_gate::MULTI_CONSUMER_DISABLED;
use crate::{
    auth::AgentCaller,
    delivery::intents::{IntentReceipt, TypingReceipt},
    media::forward::{forward_bytes, PermitSet, TOOL_BODY_DEADLINE},
    tools::{
        AttachmentStatus, AttachmentView, AttachmentViewKind, ContextResponse, ToolsState,
        BUDGET_ACQUIRE_MAX, REST_VIEW_WEIGHT,
    },
};
use axum::{
    extract::{rejection::QueryRejection, Path, Query, RawQuery, State},
    http::{
        header::{CONTENT_TYPE, RETRY_AFTER},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cmaeg_core::{
    channel::ReplyChoice,
    ids::MessageRef,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::EventRef,
};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
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
            .route(
                "/v1/events/{event_ref}/attachments/{index}",
                get(read_event_attachment),
            )
            .with_state(state)
            .layer(RequestBodyLimitLayer::new(131072))
            .layer(axum::middleware::from_fn(protocol_headers)),
    )
}
#[utoipa::path(post,path="/v1/events/{event_ref}/replies",operation_id="replyToEvent",tag="tools",
    security(("agentBearer"=[])),
    params(("event_ref"=String,Path),
        ("destination"=Option<ReplyChoice>,Query,description="Where the reply goes on a channel with routing: `origin`, the event's own source, or `default`, the channel's default reply source, resolved when the reply is sent. Absent, the reply follows a hand-off's choice, else the origin. While multi-consumer routing is off, `default` is refused with `multi_consumer_disabled`.")),
    request_body=OutboundMessage,
    responses((status=200,body=IntentReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn reply_to_event(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    RawQuery(query): RawQuery,
    body: Result<Json<OutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let destination = match destination(query.as_deref(), state.multi_consumer_enabled) {
        Ok(value) => value,
        Err(error) => return result::<IntentReceipt>(Err(error)),
    };
    let message = match json(body) {
        Ok(value) => value,
        Err(error) => return result::<IntentReceipt>(Err(error)),
    };
    let reference = match event_ref(&reference) {
        Ok(value) => value,
        Err(error) => return result::<IntentReceipt>(Err(error)),
    };
    result(state.reply(caller, &reference, message, destination).await)
}
/// The explicit `destination` of a reply. With multi-consumer routing off,
/// `default` is refused, because delivering a message meant for a private
/// default destination to a possibly public origin would be a silent privacy
/// failure; every other value is ignored, as unknown query parameters are.
/// With it on, `origin` and `default` are honoured and anything else, a
/// repeated parameter included, is refused.
fn destination(
    query: Option<&str>,
    multi_consumer_enabled: bool,
) -> Result<Option<ReplyChoice>, Problem> {
    let values: Vec<_> = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .filter(|(name, _)| name == "destination")
        .map(|(_, value)| value)
        .collect();
    if !multi_consumer_enabled {
        if values.iter().any(|value| value == "default") {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                MULTI_CONSUMER_DISABLED,
            ));
        }
        return Ok(None);
    }
    match values.as_slice() {
        [] => Ok(None),
        [value] if value == "origin" => Ok(Some(ReplyChoice::Origin)),
        [value] if value == "default" => Ok(Some(ReplyChoice::Default)),
        _ => Err(Problem::new(
            ProblemCode::InvalidRequest,
            400,
            "destination must be origin or default",
        )),
    }
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
/// The optional query of an attachment read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentQuery {
    view: Option<AttachmentViewKind>,
    page: Option<u32>,
}
#[utoipa::path(get,path="/v1/events/{event_ref}/attachments/{index}",operation_id="readEventAttachment",tag="tools",
    security(("agentBearer"=[])),
    params(("event_ref"=String,Path),("index"=u8,Path,minimum=0,maximum=15),
        ("view"=Option<AttachmentViewKind>,Query),("page"=Option<u32>,Query,minimum=1,maximum=64)),
    responses((status=200,body=AttachmentView),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read_event_attachment(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path((reference, index)): Path<(String, String)>,
    query: Result<Query<AttachmentQuery>, QueryRejection>,
) -> Response {
    let reference = match event_ref(&reference) {
        Ok(value) => value,
        Err(error) => return result::<AttachmentView>(Err(error)),
    };
    let (Ok(index), Ok(Query(query))) = (index.parse::<u8>(), query) else {
        return result::<AttachmentView>(Err(Problem::new(
            ProblemCode::InvalidRequest,
            400,
            "invalid attachment request",
        )));
    };
    let view = query.view.unwrap_or(AttachmentViewKind::Auto);
    let page = query.page.unwrap_or(1);
    match state
        .read_attachment(caller, &reference, index, view, page)
        .await
    {
        Ok(read) => attachment_response(&state, read.view).await,
        Err(error) => result::<AttachmentView>(Err(error)),
    }
}

/// Answer `view` through the shared response budget. REST never carries
/// image bytes, so every view weighs the same.
async fn attachment_response(state: &ToolsState, view: AttachmentView) -> Response {
    let Some(media) = state.media.as_ref() else {
        return result::<AttachmentView>(Err(Problem::new(
            ProblemCode::MediaUnavailable,
            503,
            "media_disabled",
        )));
    };
    let Some(permit) = media
        .responses()
        .acquire(REST_VIEW_WEIGHT, Instant::now() + BUDGET_ACQUIRE_MAX)
        .await
    else {
        let mut response = problem(
            StatusCode::TOO_MANY_REQUESTS,
            ProblemCode::RateLimited,
            "response_budget",
        );
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        return response;
    };
    let bytes = match serde_json::to_vec(&view) {
        Ok(bytes) => bytes,
        Err(_) => {
            return result::<AttachmentView>(Err(Problem::new(
                ProblemCode::Internal,
                500,
                "tool response unavailable",
            )))
        }
    };
    let mut permits = PermitSet::new();
    permits.push(permit);
    let mut response =
        forward_bytes(bytes.into(), permits, Instant::now() + TOOL_BODY_DEADLINE).into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let (AttachmentStatus::Pending, Some(after)) = (view.status, view.retry_after_ms) {
        headers.insert(RETRY_AFTER, HeaderValue::from(after.div_ceil(1000)));
    }
    response
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
