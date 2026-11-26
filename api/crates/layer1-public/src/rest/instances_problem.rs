use axum::{http::StatusCode, response::IntoResponse, response::Response};
use layer2_core::sandbox::{
    HierarchyReadError, SandboxLifecycleBlocker, SandboxLifecycleCommandError, SandboxServiceError,
    WorkspaceServiceError,
};

use super::{problem_document, problem_response};

pub(crate) enum HierarchyProblem {
    AdmissionPaused,
    ResourceAccess(layer2_core::resource_access::ResourceAccessError),
    CredentialSetupRequired,
    ConnectionsRequired(Vec<layer2_core::ServiceId>),
    Forbidden,
    AlwaysOnApprovalRequired,
    SandboxNotFound,
    WorkspaceNotFound,
    WorktreeNotFound,
    ProfileNotFound(&'static str),
    TrustConfirmationRequired(String),
    LimitReached {
        resource: &'static str,
        limit: u64,
        live: u64,
        quota_usage: Option<layer2_core::sandbox::SandboxQuotaUsage>,
    },
    Conflict(&'static str),
    Invalid(&'static str),
    InvalidQuery,
    InvalidPage,
    Unavailable,
    GlobalCapacityReached,
    Budget(layer2_core::budget::BudgetError),
    SandboxLifecycleUnavailable(&'static str),
    SandboxLifecycleBusy(SandboxLifecycleBlocker),
}

impl HierarchyProblem {
    pub(crate) fn connection(
        error: layer2_core::sandbox::SandboxCredentialPrerequisiteError,
    ) -> Self {
        use layer2_core::sandbox::SandboxCredentialPrerequisiteError as Error;
        match error {
            Error::SetupRequired => Self::CredentialSetupRequired,
            Error::ConnectionsRequired { services } => Self::ConnectionsRequired(services),
            Error::Unavailable(_) => Self::Unavailable,
        }
    }
    pub(crate) fn sandbox(error: SandboxServiceError) -> Self {
        match error {
            SandboxServiceError::AccountDeletionInProgress => Self::ResourceAccess(
                layer2_core::resource_access::ResourceAccessError::AccountDeletionInProgress,
            ),
            SandboxServiceError::GlobalCapacityReached => Self::GlobalCapacityReached,
            SandboxServiceError::Budget(reason) => Self::Budget(reason),
            SandboxServiceError::AdmissionPaused => Self::AdmissionPaused,
            SandboxServiceError::CredentialSetupRequired => Self::CredentialSetupRequired,
            SandboxServiceError::ConnectionsRequired { services } => {
                Self::ConnectionsRequired(services)
            }
            SandboxServiceError::Forbidden => Self::Forbidden,
            SandboxServiceError::AlwaysOnApprovalRequired => Self::AlwaysOnApprovalRequired,
            SandboxServiceError::ProfileNotFound => Self::ProfileNotFound("sandbox"),
            SandboxServiceError::NotFound => Self::SandboxNotFound,
            SandboxServiceError::TrustConfirmationRequired { commands_digest } => {
                Self::TrustConfirmationRequired(commands_digest)
            }
            SandboxServiceError::SandboxLimitReached { limit, live, usage } => Self::LimitReached {
                resource: "sandbox",
                limit: u64::from(limit),
                live: u64::from(live),
                quota_usage: Some(usage),
            },
            SandboxServiceError::InvalidName => Self::Invalid("invalid_sandbox_name"),
            SandboxServiceError::NameConflict => Self::Conflict("sandbox_name_conflict"),
            SandboxServiceError::SandboxOriginConflict => Self::Conflict("sandbox_origin_conflict"),
            SandboxServiceError::InvalidOrigin => Self::Invalid("invalid_sandbox_origin"),
            SandboxServiceError::InvalidState => Self::Conflict("sandbox_invalid_state"),
            SandboxServiceError::InvalidServiceAccess => {
                Self::Invalid("invalid_sandbox_service_access")
            }
            SandboxServiceError::ServiceAccessRequiresPause => {
                Self::Conflict("sandbox_service_access_requires_pause")
            }
            SandboxServiceError::Corrupt(_) | SandboxServiceError::Unavailable(_) => {
                Self::Unavailable
            }
        }
    }

    pub(crate) fn workspace(error: WorkspaceServiceError) -> Self {
        match error {
            WorkspaceServiceError::Connection(error) => Self::connection(error),
            WorkspaceServiceError::Forbidden => Self::Forbidden,
            WorkspaceServiceError::ProfileNotFound => Self::ProfileNotFound("workspace"),
            WorkspaceServiceError::NotFound => Self::WorkspaceNotFound,
            WorkspaceServiceError::SandboxNotReady => Self::Conflict("sandbox_not_ready"),
            WorkspaceServiceError::NameConflict => Self::Conflict("workspace_name_conflict"),
            WorkspaceServiceError::LimitReached { limit, live } => Self::LimitReached {
                resource: "workspace",
                limit: u64::from(limit),
                live: u64::from(live),
                quota_usage: None,
            },
            WorkspaceServiceError::HasLiveAgents => Self::Conflict("workspace_has_live_agents"),
            WorkspaceServiceError::TrustConfirmationRequired { digest } => {
                Self::TrustConfirmationRequired(digest)
            }
            WorkspaceServiceError::InvalidProfile(_) => Self::Invalid("workspace_profile_invalid"),
            WorkspaceServiceError::OriginConflict => Self::Conflict("workspace_origin_conflict"),
            WorkspaceServiceError::RecoveryConflict => {
                Self::Conflict("workspace_prepare_recovery_conflict")
            }
            WorkspaceServiceError::InvalidOrigin => Self::Invalid("invalid_workspace_origin"),
            WorkspaceServiceError::Budget(reason) => Self::Budget(reason),
            WorkspaceServiceError::AdmissionPaused => Self::AdmissionPaused,
            WorkspaceServiceError::Corrupt(_) | WorkspaceServiceError::Unavailable(_) => {
                Self::Unavailable
            }
        }
    }

    pub(crate) fn sandbox_lifecycle(error: SandboxLifecycleCommandError) -> Self {
        match error {
            SandboxLifecycleCommandError::Forbidden => Self::Forbidden,
            SandboxLifecycleCommandError::NotFound => Self::SandboxNotFound,
            SandboxLifecycleCommandError::CredentialSetupRequired => Self::CredentialSetupRequired,
            SandboxLifecycleCommandError::ConnectionsRequired { services } => {
                Self::ConnectionsRequired(services)
            }
            SandboxLifecycleCommandError::SandboxNotActive { .. } => {
                Self::Conflict("sandbox_not_active")
            }
            SandboxLifecycleCommandError::SandboxNotPaused { .. } => {
                Self::Conflict("sandbox_not_paused")
            }
            SandboxLifecycleCommandError::SandboxBusy { reason, .. } => {
                Self::SandboxLifecycleBusy(reason)
            }
            SandboxLifecycleCommandError::Budget(reason) => Self::Budget(reason),
            SandboxLifecycleCommandError::AdmissionPaused => Self::AdmissionPaused,
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
            HierarchyReadError::Access(error) => Self::ResourceAccess(error),
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
            Self::AdmissionPaused => return super::admission_problem::paused(),
            Self::ResourceAccess(error) => {
                return super::resource_access_problem::ResourceProblem(error).into_response()
            }
            Self::Budget(reason) => {
                return super::instance_problem::InstanceProblem::Budget(reason).into_response()
            }
            Self::ConnectionsRequired(services) => (
                StatusCode::CONFLICT,
                "connections-required",
                "Connect and verify the required services before opening this sandbox.",
                "connections_required".to_owned(),
                vec![("services", serde_json::json!(services))],
            ),
            Self::CredentialSetupRequired => (
                StatusCode::CONFLICT,
                "sandbox-credential-setup-required",
                super::problems::SANDBOX_CREDENTIAL_SETUP_DETAIL,
                "sandbox_credential_setup_required".to_owned(),
                vec![],
            ),
            Self::AlwaysOnApprovalRequired => (
                StatusCode::FORBIDDEN,
                "always-on-approval-required",
                "Always on requires approval by an administrator. Owners may only turn it off.",
                "always_on_approval_required".to_owned(),
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
                quota_usage,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "resource-limit-reached",
                "The owner has reached the live resource limit.",
                format!("{resource}_limit_reached"),
                [
                    Some(("limit", limit.into())),
                    Some(("live", live.into())),
                    quota_usage.map(|usage| {
                        (
                            "quota_usage",
                            super::sandbox_capacity::quota_usage_json(usage),
                        )
                    }),
                ]
                .into_iter()
                .flatten()
                .collect(),
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
            Self::GlobalCapacityReached => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sandbox-capacity-unavailable",
                "Sandbox capacity is temporarily unavailable. Retry the original operation later.",
                "sandbox_capacity_unavailable".to_owned(),
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

#[cfg(test)]
mod global_admission_tests {
    use super::*;

    #[tokio::test]
    async fn platform_refusals_of_new_resources_share_the_budget_problem() {
        use layer2_core::budget::BudgetError;
        for (problem, status, code) in [
            (
                HierarchyProblem::sandbox(SandboxServiceError::Budget(
                    BudgetError::PlatformExhausted,
                )),
                StatusCode::PAYMENT_REQUIRED,
                "platform_budget_exhausted",
            ),
            (
                HierarchyProblem::workspace(WorkspaceServiceError::Budget(
                    BudgetError::Unavailable,
                )),
                StatusCode::PAYMENT_REQUIRED,
                "budget_unavailable",
            ),
            (
                HierarchyProblem::sandbox_lifecycle(SandboxLifecycleCommandError::Budget(
                    BudgetError::PlatformExhausted,
                )),
                StatusCode::PAYMENT_REQUIRED,
                "platform_budget_exhausted",
            ),
        ] {
            let response = problem.into_response();
            assert_eq!(response.status(), status);
            assert!(response.headers().get("retry-after").is_none());
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], code);
        }
    }

    #[tokio::test]
    async fn global_capacity_is_retryable_and_does_not_disclose_fleet_counts() {
        let response =
            HierarchyProblem::sandbox(SandboxServiceError::GlobalCapacityReached).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "sandbox_capacity_unavailable");
        assert!(body.get("limit").is_none());
        assert!(body.get("live").is_none());
        assert_eq!(
            SandboxServiceError::GlobalCapacityReached.code(),
            body["code"]
        );
    }
}
