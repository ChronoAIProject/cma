//! Exact-message native effects; platform identifiers are never caller input.
use super::tools::{json, result};
use crate::{
    auth::AgentCaller,
    tools::{MessageEffectReceipt, MessageEffectsCapabilities, ToolsState},
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use cmaeg_core::{
    ids::MessageRef,
    message_effect::MessageReactionRequest,
    problem::{Problem, ProblemCode},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyEffectRequest {}
pub(super) fn routes() -> Router<ToolsState> {
    Router::new()
        .route(
            "/v1/messages/{message_ref}/capabilities",
            get(read_message_capabilities),
        )
        .route("/v1/messages/{message_ref}/delete", post(delete_message))
        .route(
            "/v1/messages/{message_ref}/reaction",
            post(set_message_reaction),
        )
}
#[utoipa::path(get,path="/v1/messages/{message_ref}/capabilities",operation_id="readMessageCapabilities",tag="tools",
    security(("agentBearer"=[])),params(("message_ref"=String,Path)),
    responses((status=200,body=MessageEffectsCapabilities),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read_message_capabilities(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(raw): Path<String>,
) -> Response {
    result(
        async {
            state
                .message_effect_capabilities(caller, &reference(raw)?)
                .await
        }
        .await,
    )
}
#[utoipa::path(post,path="/v1/messages/{message_ref}/delete",operation_id="deleteMessage",tag="tools",
    security(("agentBearer"=[])),params(("message_ref"=String,Path),("Idempotency-Key"=String,Header)),request_body=EmptyEffectRequest,
    responses((status=200,body=MessageEffectReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn delete_message(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Result<Json<EmptyEffectRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    result(
        async {
            json(body)?;
            state
                .delete_message(caller, &reference(raw)?, key(&headers)?)
                .await
        }
        .await,
    )
}
#[utoipa::path(post,path="/v1/messages/{message_ref}/reaction",operation_id="setMessageReaction",tag="tools",
    security(("agentBearer"=[])),params(("message_ref"=String,Path),("Idempotency-Key"=String,Header)),request_body=MessageReactionRequest,
    responses((status=200,body=MessageEffectReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn set_message_reaction(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Result<Json<MessageReactionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    result(
        async {
            state
                .set_message_reaction(caller, &reference(raw)?, key(&headers)?, json(body)?)
                .await
        }
        .await,
    )
}
fn reference(raw: String) -> Result<MessageRef, Problem> {
    MessageRef::new(raw)
        .map_err(|_| Problem::new(ProblemCode::InvalidRequest, 400, "message_ref_invalid"))
}
fn key(headers: &HeaderMap) -> Result<&str, Problem> {
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
    Ok(key)
}
