//! Input, control, history, usage, and resource routes for one CMA Agent.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::sandbox::{AgentMessageOutcome, AgentStopReason};
use layer2_core::AgentId;

use super::agent_openapi_shapes::{
    AgentTextRequest, ApprovalDecisionRequest, InputAcceptedView, StopAgentRequest,
};
use super::agents::{
    execute_command, execute_command_with_claim, execute_empty_command, AgentRoutesState,
};
use super::idempotency::{operation_scope, IdempotencyKey, RequestBody};
use super::instance_problem::InstanceProblem;
use super::BodyProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller};

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new()
        .route("/api/v1/agents/{id}/input", post(input))
        .route("/api/v1/agents/{id}/steer", post(steer))
        .route("/api/v1/agents/{id}/interrupt", post(interrupt))
        .route("/api/v1/agents/{id}/approvals/{request_id}", post(approval))
        .route("/api/v1/agents/{id}/stop", post(stop))
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/input",
    tag = "agent operations",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = AgentTextRequest,
    responses(
        (status = 202, body = InputAcceptedView),
        (status = 409, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn input(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<AgentTextRequest>, BodyProblem>,
) -> Response {
    if state.commands.is_some() {
        return super::agent_command_admission::with_body(
            &state,
            resource,
            key,
            format!("agent:input:{id}"),
            AgentId(id),
            body,
            |body| layer2_core::sandbox::agent_io::AgentCommand::message(body.text),
        )
        .await;
    }
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:input:{id}"),
        body,
        move |body| async move {
            let outcome = service
                .send_message(&operation_caller, &AgentId(id), body.text)
                .await
                .map_err(InstanceProblem::from)?;
            Ok((
                StatusCode::ACCEPTED,
                Json(InputAcceptedView {
                    queued: matches!(outcome, AgentMessageOutcome::Queued { .. }),
                }),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/steer",
    tag = "agent operations",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = AgentTextRequest,
    responses((status = 202, description = "Steering accepted"))
)]
pub(crate) async fn steer(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<AgentTextRequest>, BodyProblem>,
) -> Response {
    if state.commands.is_some() {
        return super::agent_command_admission::with_body(
            &state,
            resource,
            key,
            format!("agent:steer:{id}"),
            AgentId(id),
            body,
            |body| layer2_core::sandbox::agent_io::AgentCommand::Steer {
                input: layer2_core::sandbox::agent_io::AgentInput::text(body.text),
            },
        )
        .await;
    }
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:steer:{id}"),
        body,
        move |body| async move {
            service
                .steer(&operation_caller, &AgentId(id), body.text)
                .await
                .map_err(InstanceProblem::from)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/interrupt",
    tag = "agent operations",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, description = "Interrupt accepted"))
)]
pub(crate) async fn interrupt(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    if state.commands.is_some() {
        return super::agent_command_admission::without_body(
            &state,
            resource,
            key,
            format!("agent:interrupt:{id}"),
            AgentId(id),
            layer2_core::sandbox::agent_io::AgentCommand::Interrupt,
        )
        .await;
    }
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_empty_command(
        &state,
        &caller,
        key,
        format!("agent:interrupt:{id}"),
        move || async move {
            service
                .interrupt(&operation_caller, &AgentId(id))
                .await
                .map_err(InstanceProblem::from)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/approvals/{request_id}",
    tag = "agent operations",
    params(
        ("id" = String, Path),
        ("request_id" = String, Path),
        ("Idempotency-Key" = String, Header)
    ),
    request_body = ApprovalDecisionRequest,
    responses((status = 202, description = "Approval decision accepted"))
)]
pub(crate) async fn approval(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path((id, request_id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<ApprovalDecisionRequest>, BodyProblem>,
) -> Response {
    if state.commands.is_some() {
        return super::agent_command_admission::with_body(
            &state,
            resource,
            key,
            operation_scope("agent:approval", &[&id, &request_id]),
            AgentId(id),
            body,
            |body| layer2_core::sandbox::agent_io::AgentCommand::AnswerApproval {
                approval_id: request_id,
                decision: body.decision.into(),
            },
        )
        .await;
    }
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        operation_scope("agent:approval", &[&id, &request_id]),
        body,
        move |body| async move {
            service
                .answer_approval(
                    &operation_caller,
                    &AgentId(id),
                    request_id,
                    body.decision.into(),
                )
                .await
                .map_err(InstanceProblem::from)?;
            Ok(StatusCode::ACCEPTED.into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{id}/stop",
    tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = StopAgentRequest,
    responses((status = 202, description = "Agent stop scheduled"))
)]
pub(crate) async fn stop(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<StopAgentRequest>, BodyProblem>,
) -> Response {
    let service = state.agents.clone();
    #[cfg(feature = "qualification-hooks")]
    let qualification_state = state.clone();
    let operation_caller = caller.clone();
    let agent_id = AgentId(id.clone());
    execute_command_with_claim(
        &state,
        &caller,
        key,
        format!("agent:stop:{id}"),
        body,
        move |body, _guard| {
            #[cfg(feature = "qualification-hooks")]
            let qualification = qualification_state.qualification_for(
                _guard,
                layer2_core::qualification::ControlOperation::Stop,
                &agent_id,
            );
            async move {
                let _reason = body.reason;
                #[cfg(feature = "qualification-hooks")]
                if let Some((checkpoint, context)) = qualification {
                    service
                        .stop_with_checkpoint(
                            &operation_caller,
                            &agent_id,
                            AgentStopReason::Owner,
                            context,
                            checkpoint.as_ref(),
                        )
                        .await
                        .map_err(InstanceProblem::from_stop)?;
                    return Ok(StatusCode::ACCEPTED.into_response());
                }
                service
                    .stop(&operation_caller, &agent_id, AgentStopReason::Owner)
                    .await
                    .map_err(InstanceProblem::from_stop)?;
                Ok(StatusCode::ACCEPTED.into_response())
            }
        },
    )
    .await
}
