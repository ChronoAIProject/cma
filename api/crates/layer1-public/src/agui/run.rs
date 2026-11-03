use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{
        agent_io::{AgentCommand, AnswerContext},
        AgentApprovalDecision,
    },
    AgentId,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::{
    conversation::{commands, failure, read_failure, AnswerContextRequest},
    rest::BodyProblem,
    AgentConversationState, AuthenticatedCaller, AuthenticatedResourceCaller,
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
    #[schema(value_type = Option<ForwardedProps>)]
    forwarded_props: Option<Value>,
}

#[derive(ToSchema)]
#[allow(dead_code)]
#[serde(rename_all = "camelCase")]
struct ForwardedProps {
    answer_context: Option<AnswerContextRequest>,
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
    params(("Idempotency-Key" = Option<String>, Header)),
    responses((status = 200, body = String, content_type = "text/event-stream"),
    (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument),
    (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn run(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    headers: HeaderMap,
    body: Result<Json<RunRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    let forwarded = match body.forwarded_props {
        None | Some(Value::Null) => None,
        Some(Value::Object(mut props)) if props.keys().all(|key| key == "answerContext") => {
            props.remove("answerContext")
        }
        _ => {
            return failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_run_request",
                "This run must use an existing Agent and its frozen profile.",
            )
        }
    };
    if body
        .run_id
        .as_ref()
        .is_some_and(|id| id.trim().is_empty() || id.len() > 128)
        || body.resume.len() > 1
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
    let mut command = if let Some(resume) = body.resume.into_iter().next() {
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
        AgentCommand::AnswerApproval {
            approval_id: resume.interrupt_id,
            decision,
        }
    } else {
        AgentCommand::message(prompt.unwrap())
    };
    if let Some(context) = forwarded {
        if serde_json::from_value::<AnswerContext>(context)
            .map_err(|_| ())
            .and_then(|context| context.bind(&mut command).map_err(|_| ()))
            .is_err()
        {
            return failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_answer_context",
                "An answer context requires a message and an exact durable turn and answer.",
            );
        }
    }
    let is_message = matches!(command, AgentCommand::Message { .. });
    let submission = match commands::submission(id.clone(), command, &headers) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    if let Err(error) = state.reads.read(&caller, &id).await {
        return read_failure(error, "thread_not_found");
    }
    let watching = state.watch(&id);
    let bound = match commands::admit_and_wait(&state, resource, submission).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let after = if is_message {
        bound.turn.saturating_sub(1)
    } else {
        bound.operation_sequence
    };
    let response =
        super::replay::open_run(state, bound.resource, id, after, bound.turn, watching).await;
    commands::receipt_headers(response, &bound.receipt)
}
