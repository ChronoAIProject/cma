use std::{collections::BTreeMap, sync::Arc};

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer2_core::{CoreError, Session, SessionId, SessionSpec, SessionSurface};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::rest::{BodyProblem, Problem, SessionsState};

use super::{not_found, Conversation, OBJECT_CONVERSATION};

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CreateConversationRequest {
    /// Conversation metadata is server-owned in CMA's chat subset.
    metadata: Option<BTreeMap<String, String>>,
}

#[utoipa::path(
    post,
    path = "/api/v1/conversations",
    tag = "responses",
    request_body = CreateConversationRequest,
    responses(
        (status = 200, description = "Chat conversation created", body = Conversation),
        (status = 422, description = "Conversation metadata is outside the declared subset", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn create(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<Extension<DelegatedCredential>>,
    body: Result<Json<CreateConversationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return BodyProblem::from(error).into_response(),
    };
    if body
        .metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_empty())
    {
        return super::field_refusal(
            "metadata",
            "Conversation metadata is server-owned; CMA derives metadata.title from the chat thread.",
        );
    }

    let service = Arc::clone(&state.sessions);
    let creation_caller = caller.clone();
    let creation = tokio::spawn(async move {
        service
            .create_session(
                &creation_caller,
                SessionSpec {
                    surface: Some(SessionSurface::Chat),
                    options: Default::default(),
                    geometry: None,
                    environment_profile: None,
                    caller_credential: credential
                        .map(|Extension(value)| value.into_caller_credential()),
                    runtime_credential: None,
                    requested_mode: None,
                    quoted_rate_card: None,
                },
            )
            .await
    });
    // Dropping a JoinHandle detaches its task. Once creation begins, a closed
    // tab or request timeout therefore cannot strand a host slot before the
    // corresponding durable session has been committed or rolled back.
    let session = match creation.await {
        Ok(Ok(session)) => session,
        Ok(Err(CoreError::Forbidden | CoreError::SessionNotFound)) => return not_found(),
        Ok(Err(error)) => return Problem::from(error).into_response(),
        Err(error) => {
            tracing::error!(%error, "conversation creation task failed");
            return Problem::from(CoreError::RuntimeUnavailable(
                "the conversation creation task failed".into(),
            ))
            .into_response();
        }
    };
    let events = match state
        .sessions
        .events_since(&caller, &session.id, None)
        .await
    {
        Ok(events) => events,
        Err(error) => return Problem::from(error).into_response(),
    };
    Json(project(session, &events)).into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations/{id}",
    tag = "responses",
    params(("id" = String, Path, description = "Chat conversation identifier")),
    responses(
        (status = 200, description = "Chat conversation", body = Conversation),
        (status = 404, description = "Unknown, foreign, or non-chat conversation", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn retrieve(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    let id = SessionId(id);
    let session = match state.sessions.get_session(&caller, &id).await {
        Ok(session) if session.chat.is_some() => session,
        Ok(_) => return not_found(),
        Err(error) => return Problem::from(error).into_response(),
    };
    let events = match state.sessions.events_since(&caller, &id, None).await {
        Ok(events) => events,
        Err(error) => return Problem::from(error).into_response(),
    };
    Json(project(session, &events)).into_response()
}

pub(super) fn project(session: Session, events: &[layer2_core::Event]) -> Conversation {
    let created_at = events
        .iter()
        .find(|event| matches!(event.kind, layer2_core::EventKind::SessionCreated))
        .and_then(|event| layer2_core::seconds_from_rfc3339(&event.appended_at))
        .unwrap_or_default();
    let mut metadata = BTreeMap::new();
    if let Some(title) = session.chat.and_then(|chat| chat.title) {
        metadata.insert("title".into(), title);
    }
    Conversation {
        id: session.id.0,
        object: OBJECT_CONVERSATION,
        created_at,
        metadata,
    }
}
