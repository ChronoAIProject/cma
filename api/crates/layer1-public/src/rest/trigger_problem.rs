//! RFC 9457 projection for Trigger, Launch, and service-connection failures.

use axum::{http::StatusCode, response::IntoResponse, response::Response};
use layer2_core::trigger_domain::{
    LaunchServiceError, ServiceConnectionServiceError, TriggerReferenceError, TriggerServiceError,
};

use super::{problem_document, problem_response};

pub(super) enum TriggerProblem {
    Trigger(TriggerServiceError),
    Launch(LaunchServiceError),
    Connection(ServiceConnectionServiceError),
    InvalidQuery(&'static str),
    CursorBeyondEnd { head: u64 },
}

impl TriggerProblem {
    /// Hide a missing or foreign Launch behind the connection resource while
    /// preserving dependency failures as service-unavailable responses.
    pub(super) fn for_connection_launch_preflight(self) -> Self {
        match self {
            Self::Launch(LaunchServiceError::NotFound) => {
                Self::Connection(ServiceConnectionServiceError::NotFound)
            }
            problem => problem,
        }
    }
}

impl From<TriggerServiceError> for TriggerProblem {
    fn from(value: TriggerServiceError) -> Self {
        Self::Trigger(value)
    }
}

impl From<LaunchServiceError> for TriggerProblem {
    fn from(value: LaunchServiceError) -> Self {
        Self::Launch(value)
    }
}

impl From<ServiceConnectionServiceError> for TriggerProblem {
    fn from(value: ServiceConnectionServiceError) -> Self {
        Self::Connection(value)
    }
}

impl IntoResponse for TriggerProblem {
    fn into_response(self) -> Response {
        let (status, code, detail, evidence) = match self {
            Self::Launch(LaunchServiceError::VisitorQuota { limit, reset_at }) => {
                return super::LaunchQuotaError::ResourceQuota(
                    super::LaunchQuotaRefusal::visitor_launches(limit, reset_at),
                )
                .into_response();
            }
            Self::Trigger(error) => trigger_problem(error),
            Self::Launch(error) => launch_problem(error),
            Self::Connection(error) => connection_problem(error),
            Self::InvalidQuery(code) => (
                StatusCode::BAD_REQUEST,
                code,
                "The query parameters are not valid for this resource.",
                Vec::new(),
            ),
            Self::CursorBeyondEnd { head } => (
                StatusCode::BAD_REQUEST,
                "cursor_beyond_end",
                "The event cursor is beyond this Launch's current event head.",
                vec![("head", serde_json::json!(head))],
            ),
        };
        if code == "admission_paused" {
            return super::admission_problem::paused();
        }
        let slug = code.replace('_', "-");
        let mut body = problem_document(status, &slug, detail, code);
        if let Some(object) = body.as_object_mut() {
            for (name, value) in evidence {
                object.insert(name.into(), value);
            }
        }
        problem_response(status, body)
    }
}

type ProblemParts = (
    StatusCode,
    &'static str,
    &'static str,
    Vec<(&'static str, serde_json::Value)>,
);

fn trigger_problem(error: TriggerServiceError) -> ProblemParts {
    match error {
        TriggerServiceError::Forbidden => forbidden("publish or manage Triggers"),
        TriggerServiceError::NotFound => not_found("trigger_not_found", "Trigger"),
        TriggerServiceError::Duplicate => conflict(
            "trigger_already_exists",
            "A Trigger with this identity already exists.",
        ),
        TriggerServiceError::Conflict { expected, actual } => (
            StatusCode::CONFLICT,
            "trigger_draft_changed",
            "The Trigger draft changed before this operation completed.",
            vec![
                ("expected", serde_json::json!(expected)),
                ("actual", serde_json::json!(actual)),
            ],
        ),
        TriggerServiceError::AlreadyPublished => conflict(
            "trigger_already_published",
            "A Trigger that has been published cannot be deleted.",
        ),
        TriggerServiceError::Reference(error) => reference_problem(error),
        TriggerServiceError::ProfileReferenced { trigger_ids } => (
            StatusCode::CONFLICT,
            "profile_referenced_by_published_trigger",
            "A published Trigger still references this profile.",
            vec![(
                "trigger_ids",
                serde_json::json!(trigger_ids
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()),
            )],
        ),
        TriggerServiceError::Validation(error) if error.code == "placement_not_found" => {
            not_found("placement_not_found", "Placement")
        }
        TriggerServiceError::Validation(error)
            if matches!(error.code, "trigger_retired" | "trigger_not_published") =>
        {
            conflict(
                error.code,
                "The Trigger is not in the required lifecycle state.",
            )
        }
        TriggerServiceError::Validation(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            error.code,
            "The Trigger does not satisfy its validation contract.",
            vec![("field", serde_json::json!(error.field))],
        ),
        TriggerServiceError::Unavailable(_) | TriggerServiceError::Corrupt(_) => unavailable(
            "trigger_service_unavailable",
            "The Trigger service could not complete this operation.",
        ),
    }
}

fn launch_problem(error: LaunchServiceError) -> ProblemParts {
    match error {
        LaunchServiceError::AccountDeletionInProgress => (
            StatusCode::FORBIDDEN,
            "account_deletion_in_progress",
            "Account deletion is in progress.",
            vec![],
        ),
        LaunchServiceError::VisitorQuota { .. } => {
            unreachable!("quota response carries Retry-After above")
        }
        LaunchServiceError::CredentialSetupRequired => conflict(
            "sandbox_credential_setup_required",
            super::problems::SANDBOX_CREDENTIAL_SETUP_DETAIL,
        ),
        LaunchServiceError::Forbidden => forbidden("launch Triggers"),
        LaunchServiceError::Disabled => unavailable(
            "triggers_disabled",
            "Trigger launches are disabled by the operator.",
        ),
        LaunchServiceError::TriggerNotFound => not_found("trigger_not_found", "Trigger"),
        LaunchServiceError::TriggerRetired => conflict(
            "trigger_retired",
            "This Trigger is retired and no longer accepts launches. Choose another Trigger to start a new Launch.",
        ),
        LaunchServiceError::RepeatChoiceRequired { existing } => (
            StatusCode::CONFLICT,
            "trigger_repeat_choice_required",
            "Choose whether to continue the existing Agent or start a new one.",
            vec![(
                "existing",
                serde_json::json!({
                    "agent_id": existing.agent_id.to_string(),
                    "title": existing.title,
                    "created_at": existing.created_at,
                    "first_message_excerpt": existing.first_message_excerpt,
                }),
            )],
        ),
        LaunchServiceError::NotFound => not_found("launch_not_found", "Launch"),
        LaunchServiceError::NotStartable => conflict(
            "launch_not_startable",
            "This Launch cannot be started or cancelled from its current state.",
        ),
        LaunchServiceError::RequirementsNotMet { services } => (
            StatusCode::CONFLICT,
            "launch_requirements_not_met",
            "Required service connections are not yet satisfied.",
            vec![(
                "services",
                serde_json::json!(services
                    .into_iter()
                    .map(|service| service.to_string())
                    .collect::<Vec<_>>()),
            )],
        ),
        LaunchServiceError::TrustConfirmationMismatch => conflict(
            "trust_confirmation_mismatch",
            "The trust confirmation does not match the reviewed commands.",
        ),
        LaunchServiceError::InvalidRequest(_) => invalid_launch("invalid_launch_request"),
        LaunchServiceError::InvalidIdempotencyKey => invalid_launch("invalid_idempotency_key"),
        LaunchServiceError::IdempotencyKeyReused => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency_key_reused",
            "This Idempotency-Key is already bound to different launch input.",
            Vec::new(),
        ),
        LaunchServiceError::InvalidCursor => (
            StatusCode::BAD_REQUEST,
            "invalid_launch_cursor",
            "The Launch cursor is invalid for this query.",
            Vec::new(),
        ),
        LaunchServiceError::AdmissionPaused => (
            StatusCode::SERVICE_UNAVAILABLE,
            "admission_paused",
            layer2_core::admission::ADMISSION_PAUSED_GUIDANCE,
            Vec::new(),
        ),
        LaunchServiceError::GlobalCapacityReached => (
            StatusCode::SERVICE_UNAVAILABLE,
            "sandbox_capacity_unavailable",
            layer2_core::admission::CapacityKind::RetainedHomes.guidance(),
            Vec::new(),
        ),
        LaunchServiceError::Budget(reason) => (
            StatusCode::PAYMENT_REQUIRED,
            reason.code(),
            reason.guidance(),
            Vec::new(),
        ),
        LaunchServiceError::Conflict => conflict(
            "launch_changed",
            "The Launch changed before this operation completed.",
        ),
        LaunchServiceError::Unavailable(_) | LaunchServiceError::Corrupt(_) => unavailable(
            "launch_service_unavailable",
            "The Launch service could not complete this operation.",
        ),
    }
}

fn connection_problem(error: ServiceConnectionServiceError) -> ProblemParts {
    match error {
        ServiceConnectionServiceError::Forbidden => forbidden("connect Launch services"),
        ServiceConnectionServiceError::NotFound => {
            not_found("service_connection_not_found", "Service connection")
        }
        ServiceConnectionServiceError::InvalidRequest(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_service_connection",
            "The service connection request is invalid.",
            Vec::new(),
        ),
        ServiceConnectionServiceError::IntentReused => conflict(
            "service_connection_intent_reused",
            "This intent is already bound to a different service connection.",
        ),
        ServiceConnectionServiceError::Conflict => conflict(
            "service_connection_changed",
            "The service connection changed before it could be saved.",
        ),
        ServiceConnectionServiceError::Unavailable(_)
        | ServiceConnectionServiceError::Corrupt(_) => unavailable(
            "service_connection_unavailable",
            "The service connection provider could not complete this operation.",
        ),
    }
}

fn reference_problem(error: TriggerReferenceError) -> ProblemParts {
    let status = match &error {
        TriggerReferenceError::ImageAccessDenied { .. } => StatusCode::FORBIDDEN,
        TriggerReferenceError::ImageSignInRequired { .. } => StatusCode::UNAUTHORIZED,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let (code, detail, field) = match error {
        TriggerReferenceError::Unavailable { field } => (
            "trigger_reference_unavailable",
            "A referenced profile revision is unavailable to this publisher.",
            field,
        ),
        TriggerReferenceError::NotPublic { field } => (
            "trigger_reference_not_public",
            "Every profile used by a public Trigger must be public.",
            field,
        ),
        TriggerReferenceError::ImageAccessDenied { field } => (
            "trigger_image_access_denied",
            "The selected Image Manager image is not accessible to this publisher.",
            field,
        ),
        TriggerReferenceError::ImageSignInRequired { field } => (
            "trigger_image_sign_in_required",
            "Sign in again to authorize the selected Image Manager image.",
            field,
        ),
        TriggerReferenceError::ImageSelectionUnsupported { field } => (
            "trigger_image_selection_unsupported",
            "New custom image selections are unavailable.",
            field,
        ),
    };
    (
        status,
        code,
        detail,
        vec![("field", serde_json::json!(field))],
    )
}

fn forbidden(action: &'static str) -> ProblemParts {
    (
        StatusCode::FORBIDDEN,
        "forbidden",
        match action {
            "publish or manage Triggers" => {
                "This caller is not allowed to publish or manage Triggers."
            }
            "launch Triggers" => "This caller is not allowed to launch Triggers.",
            _ => "This caller is not allowed to connect Launch services.",
        },
        Vec::new(),
    )
}

fn not_found(code: &'static str, resource: &'static str) -> ProblemParts {
    (
        StatusCode::NOT_FOUND,
        code,
        match resource {
            "Trigger" => "No Trigger matches this request.",
            "Launch" => "No Launch matches this request.",
            "Placement" => "No Placement matches this request.",
            _ => "No service connection matches this request.",
        },
        Vec::new(),
    )
}

fn conflict(code: &'static str, detail: &'static str) -> ProblemParts {
    (StatusCode::CONFLICT, code, detail, Vec::new())
}

fn invalid_launch(code: &'static str) -> ProblemParts {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        code,
        "The Launch request does not satisfy its validation contract.",
        Vec::new(),
    )
}

fn unavailable(code: &'static str, detail: &'static str) -> ProblemParts {
    (StatusCode::SERVICE_UNAVAILABLE, code, detail, Vec::new())
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn launch_home_capacity_refusal_exposes_only_safe_guidance() {
        let response =
            TriggerProblem::from(LaunchServiceError::GlobalCapacityReached).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "sandbox_capacity_unavailable");
        assert_eq!(
            body["detail"],
            layer2_core::admission::CapacityKind::RetainedHomes.guidance()
        );
        assert_eq!(body.as_object().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn connection_preflight_conceals_only_an_absent_launch() {
        let missing = TriggerProblem::from(LaunchServiceError::NotFound)
            .for_connection_launch_preflight()
            .into_response();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(missing.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "service_connection_not_found");

        let unavailable = TriggerProblem::from(LaunchServiceError::Unavailable(
            "private storage detail".into(),
        ))
        .for_connection_launch_preflight()
        .into_response();
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(unavailable.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "launch_service_unavailable");
        assert!(!body.to_string().contains("private storage detail"));
    }
}
