use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::{AgentOperationError, AgentStatus};
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
    conversation::{context_failure, read_failure},
    rest::{instance_problem::InstanceProblem, BodyProblem},
    AgentConversationState, AuthenticatedCaller,
};

#[utoipa::path(post, path = "/api/v1/responses", tag = "responses", request_body = CreateResponseRequest,
    responses((status = 200, body = String, content_type = "text/event-stream"),
    (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn create(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
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
    let agent = match state.reads.read(&caller, &request.conversation).await {
        Ok(agent) => agent,
        Err(error) => return read_failure(error, "conversation_not_found"),
    };
    let task_state = state.clone();
    let watching = state.watch(&agent.id);
    let task_caller = caller.clone();
    let task_id = request.conversation.clone();
    let admission = tokio::spawn(async move {
        task_state
            .context
            .await_input_ready(&task_caller, &task_id)
            .await
            .map_err(context_failure)?;
        task_state
            .operations
            .send_linear_message(
                &task_caller,
                &task_id,
                request.input,
                previous,
                request.metadata,
            )
            .await
            .map_err(operation_failure)
    });
    match admission.await {
        Ok(Ok(receipt)) => super::stream::open(
            state,
            caller,
            receipt.agent,
            receipt.turn_sequence,
            watching,
        ),
        Ok(Err(response)) if response.status() == axum::http::StatusCode::GATEWAY_TIMEOUT => {
            super::stream::provisioning_timeout(agent)
        }
        Ok(Err(response)) => response,
        Err(error) => {
            tracing::error!(%error, "Responses admission task failed");
            InstanceProblem::Unavailable.into_response()
        }
    }
}

fn operation_failure(error: AgentOperationError) -> Response {
    match error {
        AgentOperationError::PreviousTurnConflict => super::previous_conflict(),
        AgentOperationError::StatusConflict {
            status: AgentStatus::Working | AgentStatus::WaitingApproval,
        }
        | AgentOperationError::DispatchInProgress { .. } => super::response_in_progress(),
        other => InstanceProblem::from(other).into_response(),
    }
}
