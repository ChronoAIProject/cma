use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::profiles::authoring::ProfileAuthoringError;

use super::super::{problem_document, problem_response};

pub(in super::super) struct AuthoringProblem(ProfileAuthoringError);

impl From<ProfileAuthoringError> for AuthoringProblem {
    fn from(error: ProfileAuthoringError) -> Self {
        Self(error)
    }
}

impl IntoResponse for AuthoringProblem {
    fn into_response(self) -> Response {
        let (status, code, detail) = match self.0 {
            ProfileAuthoringError::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "This caller is not allowed to manage profiles.",
            ),
            ProfileAuthoringError::CatalogForbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Reading your catalog requires permission to use your own Sandboxes or manage your own profiles.",
            ),
            ProfileAuthoringError::InvalidQuery => (
                StatusCode::BAD_REQUEST,
                "invalid_profile_authoring_query",
                "The profile authoring query is invalid.",
            ),
            ProfileAuthoringError::InvalidRepository(error) => {
                let mut body = problem_document(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &error.code.replace('_', "-"),
                    "The repository URL or reference is invalid.",
                    error.code,
                );
                body["field"] = serde_json::json!(error.field);
                return problem_response(StatusCode::UNPROCESSABLE_ENTITY, body);
            }
            ProfileAuthoringError::RepositoryNotFound => (
                StatusCode::NOT_FOUND,
                "repository_not_found",
                "No public repository matches this request.",
            ),
            ProfileAuthoringError::ReferenceNotFound => (
                StatusCode::NOT_FOUND,
                "repository_reference_not_found",
                "No branch or tag matches this repository reference.",
            ),
            ProfileAuthoringError::ServicesUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "profile_services_unavailable",
                "The owner-visible service catalog is unavailable.",
            ),
            ProfileAuthoringError::SkillsUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "profile_skills_unavailable",
                "The owner-visible skill catalog is unavailable.",
            ),
            ProfileAuthoringError::SkillsAuthorizationRequired => (
                StatusCode::FORBIDDEN,
                "skill_catalog_authorization_required",
                "The current credential requires service authorization to read the skill catalog.",
            ),
            ProfileAuthoringError::RepositoryUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "repository_unavailable",
                "The repository could not be verified.",
            ),
        };
        problem_response(
            status,
            problem_document(status, &code.replace('_', "-"), detail, code),
        )
    }
}
