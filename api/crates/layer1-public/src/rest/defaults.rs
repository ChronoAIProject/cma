use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::Capability;
use serde::Serialize;
use utoipa::ToSchema;

use super::{
    instances_openapi_shapes::ProfileReferenceView, problem_document, problem_response,
    ProfilesState,
};
use crate::AuthenticatedCaller;

#[derive(Serialize, ToSchema)]
pub(super) struct DefaultProfilesView {
    sandbox_profile: ProfileReferenceView,
    workspace_profile: ProfileReferenceView,
    agent_profile: ProfileReferenceView,
}

#[utoipa::path(get, path = "/api/v1/defaults", tag = "profiles",
    responses((status = 200, body = DefaultProfilesView),
    (status = 401, body = super::ProblemDocument), (status = 403, body = super::ProblemDocument),
    (status = 503, body = super::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Response {
    if caller.require(Capability::UseOwnSandboxes).is_err() {
        return problem_response(
            StatusCode::FORBIDDEN,
            problem_document(
                StatusCode::FORBIDDEN,
                "forbidden",
                "This caller is not allowed to use owned Sandboxes.",
                "forbidden",
            ),
        );
    }
    let stored = match state.settings.current().await {
        Ok(stored) => stored,
        Err(_) => {
            return problem_response(
                StatusCode::SERVICE_UNAVAILABLE,
                problem_document(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "defaults-unavailable",
                    "The current deployment defaults are unavailable.",
                    "defaults_unavailable",
                ),
            )
        }
    };
    let defaults = stored.settings.defaults;
    Json(DefaultProfilesView {
        sandbox_profile: ProfileReferenceView {
            id: defaults.sandbox_profile.id.to_string(),
            revision: defaults.sandbox_profile.revision,
        },
        workspace_profile: ProfileReferenceView {
            id: defaults.workspace_profile.id.to_string(),
            revision: defaults.workspace_profile.revision,
        },
        agent_profile: ProfileReferenceView {
            id: defaults.agent_profile.id.to_string(),
            revision: defaults.agent_profile.revision,
        },
    })
    .into_response()
}
