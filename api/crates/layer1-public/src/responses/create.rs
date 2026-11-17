use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::agent_io::{AgentCommand, AgentInput, AgentInputOrder};
use serde_json::Value;

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(super) struct CreateResponseRequest {
    conversation: String,
    input: Value,
    stream: bool,
    metadata: Option<std::collections::BTreeMap<String, String>>,
    previous_response_id: Option<String>,
    model: Option<String>,
    store: Option<bool>,
}

use crate::{
    conversation::{commands, read_failure},
    rest::BodyProblem,
    AgentConversationState, AuthenticatedCaller, AuthenticatedResourceCaller,
};

#[utoipa::path(post, path = "/api/v1/responses", tag = "responses", request_body = CreateResponseRequest,
    params(("Idempotency-Key" = Option<String>, Header)),
    responses((status = 200, body = String, content_type = "text/event-stream"),
    (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn create(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    let request = match super::parse::parse(body) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    let previous = match request.previous_response_id {
        Some(previous) => match super::retrieve::parse_response_id(&previous) {
            Some((id, turn)) if id == request.conversation => Some(turn),
            _ => return super::previous_conflict(),
        },
        None => None,
    };
    let command = AgentCommand::Message {
        input: AgentInput::text(request.input),
        order: AgentInputOrder::AfterTurn {
            turn_sequence: previous,
        },
        metadata: request.metadata,
    };
    let submission = match commands::submission(request.conversation.clone(), command, &headers) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let agent = match state.reads.read(&caller, &request.conversation).await {
        Ok(agent) => agent,
        Err(error) => return read_failure(error, "conversation_not_found"),
    };
    let watching = match state.watch(&agent.id).await {
        Ok(watching) => watching,
        Err(response) => return response,
    };
    let bound = match commands::admit_and_wait(&state, resource, submission).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let response =
        super::stream::open_authorized(state, bound.resource, agent, bound.turn, watching).await;
    commands::receipt_headers(response, &bound.receipt)
}
