use axum::{http::StatusCode, response::IntoResponse, response::Response};
use layer2_core::sandbox::{
    HierarchyReadError, SandboxLifecycleBlocker, SandboxLifecycleCommandError, SandboxServiceError,
    WorkspaceServiceError,
};

use super::{problem_document, problem_response};

pub(crate) enum HierarchyProblem {
    CredentialSetupRequired,
    Forbidden,
    SandboxNotFound,
    WorkspaceNotFound,
    WorktreeNotFound,
    ProfileNotFound(&'static str),
    TrustConfirmationRequired(String),
    LimitReached {
        resource: &'static str,
        limit: u64,
        live: u64,
    },
    Conflict(&'static str),
    Invalid(&'static str),
    InvalidQuery,
    InvalidPage,
    Unavailable,
    SandboxLifecycleUnavailable(&'static str),
    SandboxLifecycleBusy(SandboxLifecycleBlocker),
}

impl HierarchyProblem {
    pub(crate) fn sandbox(error: SandboxServiceError) -> Self {
        match error {
            SandboxServiceError::CredentialSetupRequired => Self::CredentialSetupRequired,
            SandboxServiceError::Forbidden => Self::Forbidden,
            SandboxServiceError::ProfileNotFound => Self::ProfileNotFound("sandbox"),
            SandboxServiceError::NotFound => Self::SandboxNotFound,
            SandboxServiceError::TrustConfirmationRequired { commands_digest } => {
                Self::TrustConfirmationRequired(commands_digest)
            }
            SandboxServiceError::SandboxLimitReached { limit, live } => Self::LimitReached {
                resource: "sandbox",
                limit: u64::from(limit),
                live: u64::from(live),
            },
            SandboxServiceError::InvalidName => Self::Invalid("invalid_sandbox_name"),
            SandboxServiceError::NameConflict => Self::Conflict("sandbox_name_conflict"),
            SandboxServiceError::SandboxOriginConflict => Self::Conflict("sandbox_origin_conflict"),
            SandboxServiceError::InvalidOrigin => Self::Invalid("invalid_sandbox_origin"),
            SandboxServiceError::InvalidState => Self::Conflict("sandbox_invalid_state"),
            SandboxServiceError::Corrupt(_) | SandboxServiceError::Unavailable(_) => {
                Self::Unavailable
            }
        }
    }

    pub(crate) fn workspace(error: WorkspaceServiceError) -> Self {
        match error {
            WorkspaceServiceError::Forbidden => Self::Forbidden,
            WorkspaceServiceError::ProfileNotFound => Self::ProfileNotFound("workspace"),
            WorkspaceServiceError::NotFound => Self::WorkspaceNotFound,
            WorkspaceServiceError::SandboxNotReady => Self::Conflict("sandbox_not_ready"),
            WorkspaceServiceError::NameConflict => Self::Conflict("workspace_name_conflict"),
            WorkspaceServiceError::LimitReached { limit, live } => Self::LimitReached {
                resource: "workspace",
                limit: u64::from(limit),
                live: u64::from(live),
            },
            WorkspaceServiceError::HasLiveAgents => Self::Conflict("workspace_has_live_agents"),
            WorkspaceServiceError::TrustConfirmationRequired { digest } => {
                Self::TrustConfirmationRequired(digest)
            }
            WorkspaceServiceError::InvalidProfile(_) => Self::Invalid("workspace_profile_invalid"),
            WorkspaceServiceError::OriginConflict => Self::Conflict("workspace_origin_conflict"),
            WorkspaceServiceError::InvalidOrigin => Self::Invalid("invalid_workspace_origin"),
            WorkspaceServiceError::Corrupt(_) | WorkspaceServiceError::Unavailable(_) => {
                Self::Unavailable
            }
        }
    }

    pub(crate) fn sandbox_lifecycle(error: SandboxLifecycleCommandError) -> Self {
        match error {
            SandboxLifecycleCommandError::Forbidden => Self::Forbidden,
            SandboxLifecycleCommandError::NotFound => Self::SandboxNotFound,
            SandboxLifecycleCommandError::SandboxNotActive { .. } => {
                Self::Conflict("sandbox_not_active")
            }
            SandboxLifecycleCommandError::SandboxNotPaused { .. } => {
                Self::Conflict("sandbox_not_paused")
            }
            SandboxLifecycleCommandError::SandboxBusy { reason, .. } => {
                Self::SandboxLifecycleBusy(reason)
            }
            SandboxLifecycleCommandError::Corrupt(_) => {
                Self::SandboxLifecycleUnavailable("stored_sandbox_corrupt")
            }
            SandboxLifecycleCommandError::Unavailable(_) => {
                Self::SandboxLifecycleUnavailable("sandbox_lifecycle_unavailable")
            }
        }
    }

    pub(super) fn sandbox_read(error: HierarchyReadError) -> Self {
        Self::read(error, Self::SandboxNotFound)
    }

    pub(super) fn workspace_read(error: HierarchyReadError) -> Self {
        Self::read(error, Self::WorkspaceNotFound)
    }

    pub(super) fn worktree_read(error: HierarchyReadError) -> Self {
        Self::read(error, Self::WorktreeNotFound)
    }

    fn read(error: HierarchyReadError, not_found: Self) -> Self {
        match error {
            HierarchyReadError::Forbidden => Self::Forbidden,
            HierarchyReadError::NotFound => not_found,
            HierarchyReadError::InvalidPage => Self::InvalidPage,
            HierarchyReadError::Corrupt(_) | HierarchyReadError::Unavailable(_) => {
                Self::Unavailable
            }
        }
    }
}

impl IntoResponse for HierarchyProblem {
    fn into_response(self) -> Response {
        let (status, slug, detail, code, fields) = match self {
            Self::CredentialSetupRequired => (
                StatusCode::CONFLICT,
                "sandbox-credential-setup-required",
                super::problems::SANDBOX_CREDENTIAL_SETUP_DETAIL,
                "sandbox_credential_setup_required".to_owned(),
                vec![],
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "This caller is not allowed to use this resource.",
                "forbidden".to_owned(),
                vec![],
            ),
            Self::SandboxNotFound => not_found("sandbox"),
            Self::WorkspaceNotFound => not_found("workspace"),
            Self::WorktreeNotFound => not_found("worktree"),
            Self::ProfileNotFound(resource) => (
                StatusCode::NOT_FOUND,
                "profile-not-found",
                "No usable profile revision matches this request.",
                format!("{resource}_profile_not_found"),
                vec![],
            ),
            Self::TrustConfirmationRequired(digest) => (
                StatusCode::CONFLICT,
                "trust-confirmation-required",
                "Confirm the exact commands digest before creating this resource.",
                "trust_confirmation_required".to_owned(),
                vec![("commands_digest", serde_json::Value::String(digest))],
            ),
            Self::LimitReached {
                resource,
                limit,
                live,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "resource-limit-reached",
                "The owner has reached the live resource limit.",
                format!("{resource}_limit_reached"),
                vec![("limit", limit.into()), ("live", live.into())],
            ),
            Self::Conflict(code) => (
                StatusCode::CONFLICT,
                "resource-conflict",
                "The operation conflicts with current resource state.",
                code.to_owned(),
                vec![],
            ),
            Self::Invalid(code) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-resource-request",
                "The request does not satisfy the resource contract.",
                code.to_owned(),
                vec![],
            ),
            Self::InvalidQuery => (
                StatusCode::BAD_REQUEST,
                "invalid-query",
                "The query parameters are not valid.",
                "invalid_query".to_owned(),
                vec![],
            ),
            Self::InvalidPage => (
                StatusCode::BAD_REQUEST,
                "invalid-page",
                "The page or event cursor parameters are not valid.",
                "invalid_page".to_owned(),
                vec![],
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "hierarchy-service-unavailable",
                "The resource service could not complete this operation.",
                "hierarchy_service_unavailable".to_owned(),
                vec![],
            ),
            Self::SandboxLifecycleUnavailable(code) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sandbox-lifecycle-unavailable",
                "The Sandbox lifecycle service could not complete this operation.",
                code.to_owned(),
                vec![],
            ),
            Self::SandboxLifecycleBusy(reason) => (
                StatusCode::CONFLICT,
                "sandbox-lifecycle-busy",
                "The Sandbox is completing lifecycle work. Refresh its state before retrying.",
                "sandbox_lifecycle_busy".to_owned(),
                vec![("reason", serde_json::json!(reason))],
            ),
        };
        let mut body = problem_document(status, slug, detail, &code);
        if let Some(object) = body.as_object_mut() {
            object.extend(
                fields
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value)),
            );
        }
        problem_response(status, body)
    }
}

type ProblemParts = (
    StatusCode,
    &'static str,
    &'static str,
    String,
    Vec<(&'static str, serde_json::Value)>,
);

fn not_found(resource: &'static str) -> ProblemParts {
    (
        StatusCode::NOT_FOUND,
        "resource-not-found",
        "No resource matches this request.",
        format!("{resource}_not_found"),
        vec![],
    )
}
