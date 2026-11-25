use super::{agents::AgentRoutesState, instance_problem::InstanceProblem};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{AgentReplyPreview, AssistantReplyPreview},
    AgentId,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct AssistantReplyPreviewView {
    text: String,
    truncated: bool,
    message_sequence: u64,
    turn_sequence: u64,
    appended_at: String,
}

impl From<AssistantReplyPreview> for AssistantReplyPreviewView {
    fn from(value: AssistantReplyPreview) -> Self {
        Self {
            text: value.text,
            truncated: value.truncated,
            message_sequence: value.message_sequence,
            turn_sequence: value.turn_sequence,
            appended_at: value.appended_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum ReplyPreviewResponse {
    Ready {
        agent_id: String,
        source_through_seq: u64,
        latest_assistant_reply: AssistantReplyPreviewView,
    },
    None {
        agent_id: String,
        source_through_seq: u64,
        latest_assistant_reply: Option<AssistantReplyPreviewView>,
    },
    Unavailable {
        agent_id: String,
        source_through_seq: Option<u64>,
        latest_assistant_reply: Option<AssistantReplyPreviewView>,
    },
}

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new().route("/api/v1/agents/{id}/reply-preview", get(read))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/reply-preview", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = ReplyPreviewResponse),
        (status = 401, description = "Authentication required"),
        (status = 403, description = "History permission required"),
        (status = 404, description = "Agent is not visible"),
        (status = 503, description = "Reply source cannot be read safely")))]
pub(super) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Response, InstanceProblem> {
    let result = state
        .reads
        .reply_preview(
            &caller,
            resource.as_ref().map(|r| &r.context),
            resource
                .as_ref()
                .and_then(|r| r.credential.as_ref().map(DelegatedCredential::value)),
            &AgentId(id.clone()),
        )
        .await?;
    Ok(response(id, result))
}

fn response(agent_id: String, result: AgentReplyPreview) -> Response {
    let body = match result {
        AgentReplyPreview::Ready {
            source_through_seq,
            reply,
        } => ReplyPreviewResponse::Ready {
            agent_id,
            source_through_seq,
            latest_assistant_reply: reply.into(),
        },
        AgentReplyPreview::None { source_through_seq } => ReplyPreviewResponse::None {
            agent_id,
            source_through_seq,
            latest_assistant_reply: None,
        },
        AgentReplyPreview::Unavailable => ReplyPreviewResponse::Unavailable {
            agent_id,
            source_through_seq: None,
            latest_assistant_reply: None,
        },
    };
    ([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response()
}

#[cfg(test)]
pub(crate) mod tests;
