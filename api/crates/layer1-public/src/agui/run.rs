use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{AgentApprovalDecision, AgentMessageOutcome, AgentStatus},
    AgentId,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::{
    conversation::{context_failure, failure, read_failure},
    rest::{instance_problem::InstanceProblem, BodyProblem},
    AgentConversationState, AuthenticatedCaller,
};

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunRequest {
    thread_id: String,
    run_id: Option<String>,
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    resume: Vec<Resume>,
    forwarded_props: Option<Value>,
}

#[derive(Deserialize, ToSchema)]
struct Message {
    role: String,
    content: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct Resume {
    interrupt_id: String,
    status: Option<String>,
    payload: Option<ResumePayload>,
}

#[derive(Deserialize, ToSchema)]
struct ResumePayload {
    decision: Option<String>,
}

#[utoipa::path(post, path = "/api/v1/agui/run", tag = "ag-ui", request_body = RunRequest,
    responses((status = 200, body = String, content_type = "text/event-stream"),
    (status = 404, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn run(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<RunRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    if body
        .run_id
        .as_ref()
        .is_some_and(|id| id.trim().is_empty() || id.len() > 128)
        || body.resume.len() > 1
        || body
            .forwarded_props
            .as_ref()
            .is_some_and(|value| !value.is_null() && value != &serde_json::json!({}))
    {
        return failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_run_request",
            "This run must use an existing Agent and its frozen profile.",
        );
    }
    let prompt = body
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .and_then(|message| message.content.clone());
    if body.resume.is_empty() && prompt.as_ref().is_none_or(|text| text.trim().is_empty()) {
        return failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_run_request",
            "A run requires a user message or one approval answer.",
        );
    }
    let id = AgentId(body.thread_id);
    if let Err(error) = state.reads.read(&caller, &id).await {
        return read_failure(error, "thread_not_found");
    }
    let task_state = state.clone();
    let watching = state.watch(&id);
    let task_caller = caller.clone();
    let task_id = id.clone();
    let admission = tokio::spawn(async move {
        let agent = task_state
            .context
            .await_input_ready(&task_caller, &task_id)
            .await
            .map_err(context_failure)?;
        let before = agent.conversation.last_seq;
        let receipt = if let Some(resume) = body.resume.into_iter().next() {
            let decision = if resume.status.as_deref() != Some("resolved") {
                AgentApprovalDecision::Deny
            } else {
                match resume
                    .payload
                    .and_then(|payload| payload.decision)
                    .as_deref()
                {
                    Some("allow") => AgentApprovalDecision::Allow,
                    Some("abort") => AgentApprovalDecision::Abort,
                    _ => AgentApprovalDecision::Deny,
                }
            };
            task_state
                .operations
                .answer_approval(&task_caller, &task_id, resume.interrupt_id, decision)
                .await
        } else if agent.status == AgentStatus::Working {
            task_state
                .operations
                .steer(&task_caller, &task_id, prompt.unwrap())
                .await
        } else {
            match task_state
                .operations
                .send_message(&task_caller, &task_id, prompt.unwrap())
                .await
            {
                Ok(AgentMessageOutcome::Dispatched(receipt)) => Ok(receipt),
                Ok(AgentMessageOutcome::Queued { .. }) => {
                    return Err(InstanceProblem::AgentNotReady.into_response())
                }
                Err(error) => Err(error),
            }
        }
        .map_err(|error| InstanceProblem::from(error).into_response())?;
        Ok::<_, Response>((receipt.turn_sequence, before))
    });
    match admission.await {
        Ok(Ok((turn, before))) => {
            super::replay::open(state, caller, id, before, true, Some(turn), watching).await
        }
        Ok(Err(response)) => response,
        Err(error) => {
            tracing::error!(%error, "AG-UI admission task failed");
            InstanceProblem::Unavailable.into_response()
        }
    }
}
