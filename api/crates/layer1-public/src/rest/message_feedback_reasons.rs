use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::message_feedback::{FeedbackDetails, FeedbackReasonError, FeedbackReasonRequest},
    AgentId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{agents::AgentRoutesState, instance_problem::InstanceProblem, BodyProblem};
use crate::AuthenticatedCaller;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetFeedbackReasonsRequest {
    expected_version: u64,
    #[schema(min_length = 1, max_length = 128, pattern = "^[A-Za-z0-9_.:-]+$")]
    request_id: String,
    #[schema(max_items = 6)]
    reasons: Vec<FeedbackReasonView>,
    #[serde(default)]
    #[schema(max_length = 500)]
    comment: String,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FeedbackReasonView {
    Inaccurate,
    Unanswered,
    UnusableSteps,
    Incomplete,
    Unclear,
    Other,
}

impl From<FeedbackReasonView> for layer2_core::sandbox::message_feedback::FeedbackReason {
    fn from(value: FeedbackReasonView) -> Self {
        match value {
            FeedbackReasonView::Inaccurate => Self::Inaccurate,
            FeedbackReasonView::Unanswered => Self::Unanswered,
            FeedbackReasonView::UnusableSteps => Self::UnusableSteps,
            FeedbackReasonView::Incomplete => Self::Incomplete,
            FeedbackReasonView::Unclear => Self::Unclear,
            FeedbackReasonView::Other => Self::Other,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct FeedbackDetailsView {
    viewer_id: String,
    agent_id: String,
    message_id: String,
    vote: Option<super::message_feedback::MessageFeedbackVote>,
    version: u64,
    #[schema(value_type = Vec<FeedbackReasonView>)]
    reasons: Vec<layer2_core::sandbox::message_feedback::FeedbackReason>,
    comment: String,
    updated_at: Option<String>,
    request_id: Option<String>,
}

fn view(
    viewer_id: String,
    agent_id: String,
    message_id: String,
    value: FeedbackDetails,
) -> FeedbackDetailsView {
    FeedbackDetailsView {
        viewer_id,
        agent_id,
        message_id,
        vote: value.vote.map(Into::into),
        version: value.version,
        reasons: value.reasons,
        comment: value.comment,
        updated_at: value.updated_at,
        request_id: value.last_request.map(|request| request.request_id),
    }
}

fn sequence(message_id: &str) -> Result<u64, InstanceProblem> {
    message_id
        .strip_prefix("event:")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && message_id == format!("event:{value}"))
        .ok_or(InstanceProblem::InvalidRequest)
}

#[utoipa::path(get, operation_id = "get_message_feedback", path = "/api/v1/agents/{id}/messages/{message_id}/feedback", tag = "agents",
    params(("id" = String, Path), ("message_id" = String, Path)),
    responses((status = 200, body = FeedbackDetailsView), (status = 401, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument),
        (status = 422, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn get(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, message_id)): Path<(String, String)>,
) -> Result<Json<FeedbackDetailsView>, Response> {
    let sequence = sequence(&message_id).map_err(IntoResponse::into_response)?;
    let service = state
        .message_feedback
        .ok_or_else(|| InstanceProblem::Unavailable.into_response())?;
    let result = service
        .get(&caller, &AgentId(id.clone()), sequence)
        .await
        .map_err(|error| InstanceProblem::from(error).into_response())?;
    Ok(Json(view(
        caller.subject.to_string(),
        id,
        message_id,
        result,
    )))
}

#[utoipa::path(put, operation_id = "set_message_feedback_reasons", path = "/api/v1/agents/{id}/messages/{message_id}/feedback/reasons", tag = "agents",
    params(("id" = String, Path), ("message_id" = String, Path)), request_body = SetFeedbackReasonsRequest,
    responses((status = 200, body = FeedbackDetailsView), (status = 401, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn set(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, message_id)): Path<(String, String)>,
    body: Result<Json<SetFeedbackReasonsRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<FeedbackDetailsView>, Response> {
    let body = body
        .map_err(|error| BodyProblem::from(error).into_response())?
        .0;
    let body = FeedbackReasonRequest {
        expected_version: body.expected_version,
        request_id: body.request_id,
        reasons: body.reasons.into_iter().map(Into::into).collect(),
        comment: body.comment,
    };
    let sequence = sequence(&message_id).map_err(IntoResponse::into_response)?;
    let service = state
        .message_feedback
        .ok_or_else(|| InstanceProblem::Unavailable.into_response())?;
    let result = service
        .set_reasons(&caller, &AgentId(id.clone()), sequence, &body)
        .await
        .map_err(|error| match error {
            FeedbackReasonError::Read(error) => InstanceProblem::from(error).into_response(),
            FeedbackReasonError::Invalid => InstanceProblem::InvalidRequest.into_response(),
            FeedbackReasonError::Conflict => super::problem_response(
                axum::http::StatusCode::CONFLICT,
                super::problem_document(
                    axum::http::StatusCode::CONFLICT,
                    "feedback-version-conflict",
                    "Read the current feedback before saving reasons.",
                    "feedback_version_conflict",
                ),
            ),
        })?;
    Ok(Json(view(
        caller.subject.to_string(),
        id,
        message_id,
        result,
    )))
}
