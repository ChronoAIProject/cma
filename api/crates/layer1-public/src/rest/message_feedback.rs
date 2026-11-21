//! Public, owner-only feedback on durable messages; independent of Agent turns.
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{sandbox::message_feedback::MessageVote, AgentId};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use super::{agents::AgentRoutesState, instance_problem::InstanceProblem, BodyProblem};
use crate::AuthenticatedCaller;

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list,
    set,
    super::message_feedback_reasons::get,
    super::message_feedback_reasons::set
))]
struct FeedbackApi;

pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    FeedbackApi::openapi()
}

#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageFeedbackVote {
    Up,
    Down,
}
impl From<MessageFeedbackVote> for MessageVote {
    fn from(value: MessageFeedbackVote) -> Self {
        match value {
            MessageFeedbackVote::Up => Self::Up,
            MessageFeedbackVote::Down => Self::Down,
        }
    }
}
impl From<MessageVote> for MessageFeedbackVote {
    fn from(value: MessageVote) -> Self {
        match value {
            MessageVote::Up => Self::Up,
            MessageVote::Down => Self::Down,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetMessageFeedbackRequest {
    /// Explicit null removes the current vote. Omitting the field is invalid.
    #[serde(deserialize_with = "deserialize_vote")]
    #[schema(required = true)]
    vote: Option<MessageFeedbackVote>,
}
fn deserialize_vote<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MessageFeedbackVote>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Serialize, ToSchema)]
pub(crate) struct MessageFeedbackView {
    /// `event:<sequence>` from the Agent transcript's message sequence.
    message_id: String,
    vote: Option<MessageFeedbackVote>,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct MessageFeedbackPageView {
    items: Vec<MessageFeedbackView>,
    /// Pass this sequence as `after` to read the next page.
    next: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeedbackQuery {
    after: Option<u64>,
    limit: Option<u16>,
}

/// List the authenticated owner's saved, non-null votes on this Agent.
#[utoipa::path(get, operation_id = "list_message_feedback",
    path = "/api/v1/agents/{id}/message-feedback", tag = "agents",
    params(("id" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = MessageFeedbackPageView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn list(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<FeedbackQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<MessageFeedbackPageView>, InstanceProblem> {
    let query = query.map_err(|_| InstanceProblem::InvalidQuery)?.0;
    let service = state.message_feedback.ok_or(InstanceProblem::Unavailable)?;
    let page = service
        .list(
            &caller,
            &AgentId(id),
            query.after.unwrap_or(0),
            query.limit.unwrap_or(200),
        )
        .await?;
    Ok(Json(MessageFeedbackPageView {
        items: page
            .items
            .into_iter()
            .map(|item| MessageFeedbackView {
                message_id: format!("event:{}", item.sequence),
                vote: Some(item.vote.into()),
            })
            .collect(),
        next: page.next,
    }))
}

/// Replace or clear the owner's vote. Idempotent metadata only: this does not
/// send input, start a turn, change conversation history or contact the runtime.
#[utoipa::path(put, operation_id = "set_message_feedback",
    path = "/api/v1/agents/{id}/messages/{message_id}/feedback", tag = "agents",
    params(("id" = String, Path), ("message_id" = String, Path)),
    request_body = SetMessageFeedbackRequest,
    responses((status = 200, body = MessageFeedbackView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 422, body = crate::rest::ProblemDocument)))]
pub(crate) async fn set(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, message_id)): Path<(String, String)>,
    body: Result<Json<SetMessageFeedbackRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<MessageFeedbackView>, Response> {
    let body = body
        .map_err(|error| BodyProblem::from(error).into_response())?
        .0;
    let sequence = message_id
        .strip_prefix("event:")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|sequence| *sequence > 0 && message_id == format!("event:{sequence}"))
        .ok_or_else(|| InstanceProblem::InvalidRequest.into_response())?;
    let service = state
        .message_feedback
        .ok_or_else(|| InstanceProblem::Unavailable.into_response())?;
    service
        .set(&caller, &AgentId(id), sequence, body.vote.map(Into::into))
        .await
        .map_err(|error| InstanceProblem::from(error).into_response())?;
    Ok(Json(MessageFeedbackView {
        message_id,
        vote: body.vote,
    }))
}
