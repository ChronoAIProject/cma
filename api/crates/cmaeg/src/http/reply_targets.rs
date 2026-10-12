//! Generic durable reply capabilities for provider integrations.
use super::tools::{event_ref, json, result};
use crate::{
    auth::AgentCaller,
    delivery::intents::IntentReceipt,
    tools::{ReplyTargetResponse, ToolsState},
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::ReplyTargetRef,
};

pub(super) fn routes() -> Router<ToolsState> {
    Router::new()
        .route(
            "/v1/events/{event_ref}/reply-target",
            post(create_reply_target),
        )
        .route(
            "/v1/reply-targets/{reply_target_ref}/messages",
            post(reply_to_target),
        )
}
#[utoipa::path(post,path="/v1/events/{event_ref}/reply-target",operation_id="createReplyTarget",tag="provider integration",
    security(("agentBearer"=[])),params(("event_ref"=String,Path)),request_body=serde_json::Value,
    responses((status=200,body=ReplyTargetResponse),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn create_reply_target(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    body: Result<Json<serde_json::Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request = async {
        let body = json(body)?;
        if !body.as_object().is_some_and(|v| v.is_empty()) {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "body must be an empty JSON object",
            ));
        }
        state
            .create_reply_target(caller, &event_ref(&reference)?)
            .await
    }
    .await;
    result(request)
}
#[utoipa::path(post,path="/v1/reply-targets/{reply_target_ref}/messages",operation_id="replyToTarget",tag="provider integration",
    security(("agentBearer"=[])),params(("reply_target_ref"=String,Path),("Idempotency-Key"=String,Header)),request_body=OutboundMessage,
    responses((status=200,body=IntentReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn reply_to_target(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    headers: HeaderMap,
    body: Result<Json<OutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request = async {
        let message = json(body)?;
        let invalid = || {
            Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "one Idempotency-Key header is required",
            )
        };
        let mut values = headers.get_all("idempotency-key").iter();
        let key = values
            .next()
            .and_then(|v| v.to_str().ok())
            .ok_or_else(invalid)?;
        if values.next().is_some() {
            return Err(invalid());
        }
        let reference: ReplyTargetRef = reference
            .parse()
            .map_err(|_| Problem::new(ProblemCode::InvalidRequest, 400, "reply_target_invalid"))?;
        state
            .reply_to_target(caller, &reference, key, message)
            .await
    }
    .await;
    result(request)
}
