//! Generic durable reply capabilities for provider integrations.
use super::tools::{event_ref, json, result};
use crate::{
    auth::AgentCaller,
    delivery::intents::IntentReceipt,
    tools::{ReplyTargetResponse, ToolsState},
    turns::routed::{AnswerReceipt, AnswerReceiptLookup, ExecutionError},
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
use serde::Deserialize;
use utoipa::ToSchema;

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
        .route(
            "/v1/reply-targets/{reply_target_ref}/accepted-answer",
            post(reply_accepted_answer),
        )
        .route(
            "/v1/accepted-answer-receipts/lookup",
            post(read_accepted_answer_receipt),
        )
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptedAnswerRequest {
    response_id: String,
    item_id: String,
    message: OutboundMessage,
}

#[utoipa::path(post,path="/v1/reply-targets/{reply_target_ref}/accepted-answer",operation_id="replyAcceptedAnswerToTarget",tag="provider integration",
    security(("agentBearer"=[])),params(("reply_target_ref"=String,Path)),request_body=AcceptedAnswerRequest,
    responses((status=200,body=IntentReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn reply_accepted_answer(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    body: Result<Json<AcceptedAnswerRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request = async {
        let request = json(body)?;
        let reference: ReplyTargetRef = reference
            .parse()
            .map_err(|_| Problem::new(ProblemCode::InvalidRequest, 400, "reply_target_invalid"))?;
        state
            .reply_accepted_answer_to_target(
                caller,
                &reference,
                crate::turns::routed::AcceptedAnswerIdentity {
                    response_id: request.response_id,
                    item_id: request.item_id,
                },
                request.message,
            )
            .await
    }
    .await;
    super::problems::no_store(result(request))
}

#[utoipa::path(post,path="/v1/accepted-answer-receipts/lookup",operation_id="readAcceptedAnswerReceipt",tag="provider integration",
    security(("agentBearer"=[])),request_body=AnswerReceiptLookup,
    responses((status=200,body=AnswerReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read_accepted_answer_receipt(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    body: Result<Json<AnswerReceiptLookup>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let read = async {
        let request = json(body)?;
        let channel = state
            .resolver
            .resolve_current(request.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        let routed = state
            .routed_replies
            .as_ref()
            .ok_or_else(answer_receipt_unavailable)?;
        routed
            .accepted_answer_receipt(caller, &channel, request)
            .await
            .map_err(|error| match error {
                ExecutionError::AuthorityChanged => {
                    Problem::new(ProblemCode::Forbidden, 403, "answer_receipt_revoked")
                }
                ExecutionError::IdempotencyConflict => {
                    Problem::new(ProblemCode::Conflict, 409, "answer_receipt_conflict")
                }
                ExecutionError::Expired => {
                    Problem::new(ProblemCode::EventRefExpired, 410, "answer_receipt_expired")
                }
                ExecutionError::Invalid => {
                    Problem::new(ProblemCode::InvalidRequest, 400, "answer_receipt_invalid")
                }
                _ => answer_receipt_unavailable(),
            })?
            .ok_or_else(|| Problem::new(ProblemCode::InvalidRequest, 404, "answer_receipt_absent"))
    }
    .await;
    super::problems::no_store(result(read))
}

fn answer_receipt_unavailable() -> Problem {
    Problem::new(ProblemCode::Internal, 503, "answer_receipt_unavailable")
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
    responses((status=200,body=IntentReceipt),(status=409,description="Admitted reply pending: retry the same Idempotency-Key and body when detail is routed_reply_in_progress",body=Problem,content_type="application/problem+json",headers(("Retry-After"=u64,description="Seconds before retrying an admitted pending reply; currently 2"))),(status="default",body=Problem,content_type="application/problem+json")))]
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
