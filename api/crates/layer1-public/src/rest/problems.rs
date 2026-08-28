//! RFC 9457 projection of the core's error vocabulary.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::CoreError;
use serde::Serialize;
use utoipa::ToSchema;

use super::PROBLEM_BASE;

/// Stable, safe classes used to choose actionable Trigger validation help.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TriggerValidationCategory {
    RepositoryIdentity,
    RepositoryPolicy,
    RepositoryRef,
    ScopeCapability,
    CommercialPolicy,
    TriggerPolicy,
}

/// RFC 9457 problem details returned by every REST refusal.
///
/// The last fields are typed extensions used by particular domain refusals.
/// They remain absent for other errors, so a generated client has one error
/// type without losing the structured facts it can act on.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProblemDocument {
    #[serde(rename = "type")]
    problem_type: String,
    title: String,
    status: u16,
    detail: String,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(inline)]
    validation_category: Option<TriggerValidationCategory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimension: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_cpu_millis: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_memory_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    free_cpu_millis: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    free_memory_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quoted_rate_card: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_card: Option<String>,
}

/// An HTTP-level refusal raised outside a core operation.
///
/// These variants are public because the composition layer can reject a
/// request before a Layer 1 handler runs. Their wire representation remains
/// owned here, beside the domain-problem renderer, so every refusal has one
/// serializer and one media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpProblem {
    /// The request body exceeded the service's accepted limit.
    RequestTooLarge,
    /// The caller exhausted the request budget for this route class.
    RateLimited {
        /// Whole seconds until one request token is expected to be available.
        retry_after_seconds: u64,
    },
    /// No REST route matched the request path.
    NotFound,
    /// The REST route exists but does not accept this method.
    MethodNotAllowed,
}

impl IntoResponse for HttpProblem {
    fn into_response(self) -> Response {
        if let Self::RateLimited {
            retry_after_seconds,
        } = self
        {
            let body = problem_document(
                StatusCode::TOO_MANY_REQUESTS,
                "rate-limited",
                "This caller has exhausted the request budget for this route.",
                "rate_limited",
            );
            let mut response = problem_response(StatusCode::TOO_MANY_REQUESTS, body);
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_str(&retry_after_seconds.max(1).to_string())
                    .expect("a positive integer is a valid Retry-After value"),
            );
            return response;
        }
        let (status, slug, detail, code) = match self {
            Self::RequestTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request-too-large",
                "The request body exceeds what this service accepts.",
                "request_too_large",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not-found",
                "No REST route matches this request path.",
                "not_found",
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                "method-not-allowed",
                "This REST route does not accept the request method.",
                "method_not_allowed",
            ),
            Self::RateLimited { .. } => unreachable!("handled above"),
        };
        let body = problem_document(status, slug, detail, code);
        problem_response(status, body)
    }
}

/// A core failure rendered as an RFC 9457 problem document.
pub struct Problem(pub(crate) CoreError);

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        // A caller who may not see a session is told it does not exist. The
        // body must render that substituted condition too, or it confirms the
        // identifier while returning a status intended to conceal it.
        let rendered = match &self.0 {
            CoreError::Forbidden => &CoreError::SessionNotFound,
            other => other,
        };
        let (status, slug) = match rendered {
            CoreError::SessionNotFound | CoreError::Forbidden => {
                (StatusCode::NOT_FOUND, "session-not-found")
            }
            CoreError::EnvironmentProfileNotFound => {
                (StatusCode::NOT_FOUND, "environment-profile-not-found")
            }
            CoreError::InvalidRequest(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-request"),
            CoreError::SessionTerminated => (StatusCode::CONFLICT, "session-terminated"),
            // A fresh request can succeed after the handover finishes on its
            // own, unlike the final session-terminated conflict. A completed
            // idempotency key still replays the refusal it recorded.
            CoreError::SessionRotating => (StatusCode::CONFLICT, "session-rotating"),
            CoreError::AlreadyAttached => (StatusCode::CONFLICT, "already-attached"),
            CoreError::InvalidGeometry(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-geometry"),
            CoreError::InvalidPressurePolicy(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid-pressure-policy")
            }
            CoreError::InvalidSessionSize(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid-session-size")
            }
            // 422 and never 409: nothing is in conflict, and stopping another
            // session does not make this one startable. The request is well
            // formed and this deployment will not provision it.
            CoreError::SizeOutOfBounds(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "size-out-of-bounds")
            }
            // The request becomes valid when the owner releases a slot. This
            // is not 429: no honest Retry-After exists for a user action.
            CoreError::SessionLimitReached { .. } => {
                (StatusCode::CONFLICT, "session-limit-reached")
            }
            CoreError::DeploymentAtCapacity { .. } => super::deployment_capacity::status_and_slug(),
            CoreError::RuntimeUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "runtime-unavailable")
            }
            // The runtime answered and refused. That is distinct from absence,
            // even though both are transient and therefore both use 503.
            CoreError::RuntimeRefused(_) => (StatusCode::SERVICE_UNAVAILABLE, "runtime-refused"),
            CoreError::InvalidState => (StatusCode::CONFLICT, "invalid-state"),
            CoreError::InitialPromptReserved(_) => {
                (StatusCode::CONFLICT, "initial-prompt-reserved")
            }
            // The route exists and the request is well formed; this session
            // simply cannot honour it, so 404 would be actively misleading.
            CoreError::CapabilityUnsupported(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "capability-unsupported")
            }
            // Retrying cannot grant a service. This is fixed by changing the
            // owner's grant, not by waiting for the service.
            CoreError::ServiceNotGranted(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "service-not-granted")
            }
            // The owner's grants are fine and the catalogue could not be read.
            // Retrying is correct, which is why this is not the refusal above.
            CoreError::ServiceCatalogUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service-catalogue-unavailable",
            ),
            CoreError::SkillCatalogUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "skill-catalogue-unavailable",
            ),
            // The same request succeeds after the caller re-quotes. Time alone
            // does not clear it, so this is a conflict rather than a rate limit.
            CoreError::PriceChanged { .. } => (StatusCode::CONFLICT, "price-changed"),
            // The document is well formed, but a sealed historical price is
            // not something this service will rewrite. Re-reading does not
            // make the same edit valid, so this is not a settings conflict.
            CoreError::PriceListNotAppendOnly(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "price-list-not-append-only",
            ),
            // Somebody else wrote first. Re-reading is the useful response,
            // which makes this a conflict rather than an invalid document.
            CoreError::SettingsConflict(_) => (StatusCode::CONFLICT, "settings-conflict"),
            CoreError::SettingsUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "settings-unavailable")
            }
        };

        // Upstream errors can contain request URLs and transport internals. The
        // logs retain those for diagnosis; the public problem says only what a
        // caller can act on and never reflects the upstream string.
        let detail = match rendered {
            CoreError::SkillCatalogUnavailable(_) => {
                "The skill catalogue could not be read completely.".to_string()
            }
            _ => rendered.to_string(),
        };
        let mut body = problem_document(status, slug, &detail, rendered.code());
        if let Some(object) = body.as_object_mut() {
            match rendered {
                // Names are fields rather than prose a client has to parse.
                CoreError::CapabilityUnsupported(name) => {
                    object.insert("capability".into(), name.clone().into());
                }
                CoreError::SizeOutOfBounds(refusal) => {
                    object.insert("dimension".into(), refusal.dimension.name().into());
                    object.insert("requested".into(), refusal.requested.into());
                    object.insert("minimum".into(), refusal.minimum.into());
                    object.insert("maximum".into(), refusal.maximum.into());
                }
                CoreError::SessionLimitReached { limit, live } => {
                    object.insert("limit".into(), (*limit).into());
                    object.insert("live".into(), (*live).into());
                }
                CoreError::DeploymentAtCapacity {
                    requested_cpu_millis,
                    requested_memory_mib,
                    free_cpu_millis,
                    free_memory_mib,
                } => super::deployment_capacity::insert_fields(
                    object,
                    *requested_cpu_millis,
                    *requested_memory_mib,
                    *free_cpu_millis,
                    *free_memory_mib,
                ),
                CoreError::PriceChanged { quoted, in_force } => {
                    object.insert("quoted_rate_card".into(), quoted.clone().into());
                    object.insert("rate_card".into(), in_force.clone().into());
                }
                _ => {}
            }
        }
        problem_response(status, body)
    }
}

impl From<CoreError> for Problem {
    fn from(error: CoreError) -> Self {
        Self(error)
    }
}

/// One RFC 9457 problem document, however the failure arose.
///
/// Shared so a request this interface could not read produces the same shape as
/// one the core refused. A caller meeting two formats from one endpoint would
/// otherwise need two parsers to find out what happened.
pub(crate) fn problem_document(
    status: StatusCode,
    slug: &str,
    detail: &str,
    code: &str,
) -> serde_json::Value {
    serde_json::to_value(ProblemDocument {
        problem_type: format!("{PROBLEM_BASE}/{slug}"),
        title: slug.replace('-', " "),
        status: status.as_u16(),
        detail: detail.to_string(),
        code: code.to_string(),
        field: None,
        validation_category: None,
        remediation: None,
        path: None,
        capability: None,
        dimension: None,
        requested: None,
        minimum: None,
        maximum: None,
        limit: None,
        live: None,
        requested_cpu_millis: None,
        requested_memory_mib: None,
        free_cpu_millis: None,
        free_memory_mib: None,
        quoted_rate_card: None,
        rate_card: None,
    })
    .expect("a problem document is serializable")
}

/// Put a completed problem document on the wire with its registered media type.
///
/// Every source of an HTTP refusal finishes here. Keeping the header beside the
/// conversion means a new problem renderer cannot accidentally inherit Axum's
/// ordinary `application/json` response type.
pub(crate) fn problem_response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{OutOfBounds, SessionSize, SizeDimension, TerminalGeometry};

    async fn rendered(error: CoreError) -> (StatusCode, serde_json::Value) {
        let response = Problem(error).into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem documents are small");
        (
            status,
            serde_json::from_slice(&bytes).expect("a problem document is json"),
        )
    }

    fn every_variant() -> Vec<CoreError> {
        let all = vec![
            CoreError::SessionNotFound,
            CoreError::EnvironmentProfileNotFound,
            CoreError::Forbidden,
            CoreError::SessionTerminated,
            CoreError::SessionRotating,
            CoreError::AlreadyAttached,
            CoreError::InvalidGeometry(TerminalGeometry::new(0, 1).unwrap_err()),
            CoreError::InvalidPressurePolicy(
                layer2_core::MemoryPressurePolicy::new(800, 780, 5, 15, 120).unwrap_err(),
            ),
            CoreError::SizeOutOfBounds(OutOfBounds {
                dimension: SizeDimension::CpuMillis,
                requested: 5_000,
                minimum: 250,
                maximum: 4_000,
            }),
            CoreError::SessionLimitReached { limit: 3, live: 3 },
            CoreError::DeploymentAtCapacity {
                requested_cpu_millis: 2_000,
                requested_memory_mib: 4_096,
                free_cpu_millis: 500,
                free_memory_mib: 1_024,
            },
            CoreError::InvalidSessionSize(SessionSize::new(0, 512).unwrap_err()),
            CoreError::RuntimeUnavailable("x".into()),
            CoreError::RuntimeRefused("x".into()),
            CoreError::InvalidState,
            CoreError::InitialPromptReserved(layer2_core::PromptDeliveryState::DispatchFenced),
            CoreError::CapabilityUnsupported("terminal".into()),
            CoreError::InvalidRequest("x".into()),
            CoreError::ServiceNotGranted("service".into()),
            CoreError::ServiceCatalogUnavailable("down".into()),
            CoreError::SkillCatalogUnavailable("down".into()),
            CoreError::PriceChanged {
                quoted: "rate_old".into(),
                in_force: "rate_new".into(),
            },
            CoreError::PriceListNotAppendOnly("card rate_example changed field 'rate'".into()),
            CoreError::SettingsConflict("x".into()),
            CoreError::SettingsUnavailable("x".into()),
        ];
        for error in &all {
            match error {
                CoreError::SessionNotFound
                | CoreError::EnvironmentProfileNotFound
                | CoreError::Forbidden
                | CoreError::SessionTerminated
                | CoreError::SessionRotating
                | CoreError::AlreadyAttached
                | CoreError::InvalidGeometry(_)
                | CoreError::InvalidPressurePolicy(_)
                | CoreError::SizeOutOfBounds(_)
                | CoreError::SessionLimitReached { .. }
                | CoreError::DeploymentAtCapacity { .. }
                | CoreError::InvalidSessionSize(_)
                | CoreError::RuntimeUnavailable(_)
                | CoreError::RuntimeRefused(_)
                | CoreError::InvalidState
                | CoreError::InitialPromptReserved(_)
                | CoreError::CapabilityUnsupported(_)
                | CoreError::InvalidRequest(_)
                | CoreError::ServiceNotGranted(_)
                | CoreError::ServiceCatalogUnavailable(_)
                | CoreError::SkillCatalogUnavailable(_)
                | CoreError::PriceChanged { .. }
                | CoreError::PriceListNotAppendOnly(_)
                | CoreError::SettingsConflict(_)
                | CoreError::SettingsUnavailable(_) => {}
            }
        }
        all
    }

    #[tokio::test]
    async fn an_out_of_bounds_size_is_unprocessable_and_carries_its_range() {
        let (status, body) = rendered(CoreError::SizeOutOfBounds(OutOfBounds {
            dimension: SizeDimension::MemoryMib,
            requested: 8_193,
            minimum: 256,
            maximum: 8_192,
        }))
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["type"]
            .as_str()
            .unwrap()
            .ends_with("/size-out-of-bounds"));
        assert_eq!(body["code"], "size_out_of_bounds");
        assert_eq!(body["dimension"], "memory_mib");
        assert_eq!(body["requested"], 8_193);
        assert_eq!(body["minimum"], 256);
        assert_eq!(body["maximum"], 8_192);
    }

    #[tokio::test]
    async fn a_skill_catalogue_failure_is_coded_and_does_not_reflect_upstream_detail() {
        let (status, body) = rendered(CoreError::SkillCatalogUnavailable(
            "GET https://registry.internal/secret?cursor=opaque failed".into(),
        ))
        .await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "skill_catalogue_unavailable");
        assert!(body["type"]
            .as_str()
            .expect("problem type")
            .ends_with("/skill-catalogue-unavailable"));
        assert_eq!(
            body["detail"],
            "The skill catalogue could not be read completely."
        );
        assert!(!body.to_string().contains("registry.internal"));
        assert!(!body.to_string().contains("opaque"));
    }

    #[tokio::test]
    async fn a_rotating_session_is_a_retryable_conflict_distinct_from_termination() {
        let (rotating_status, rotating) = rendered(CoreError::SessionRotating).await;
        let (terminated_status, terminated) = rendered(CoreError::SessionTerminated).await;

        assert_eq!(rotating_status, StatusCode::CONFLICT);
        assert_eq!(terminated_status, StatusCode::CONFLICT);
        assert_eq!(rotating["code"], "session_rotating");
        assert!(rotating["type"]
            .as_str()
            .expect("problem type")
            .ends_with("/session-rotating"));
        assert_ne!(rotating["code"], terminated["code"]);
        assert_ne!(rotating["type"], terminated["type"]);
    }

    #[tokio::test]
    async fn rewriting_a_sealed_price_is_unprocessable_and_not_a_conflict() {
        let (status, body) = rendered(CoreError::PriceListNotAppendOnly(
            "card rate_example changed field 'rate'".into(),
        ))
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["type"]
            .as_str()
            .unwrap()
            .ends_with("/price-list-not-append-only"));
        assert_eq!(body["code"], "price_list_not_append_only");
        assert!(body["detail"].as_str().unwrap().contains("rate_example"));
    }

    #[tokio::test]
    async fn a_changed_price_is_a_conflict_with_both_card_ids() {
        let (status, body) = rendered(CoreError::PriceChanged {
            quoted: "rate_old".into(),
            in_force: "rate_new".into(),
        })
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["type"].as_str().unwrap().ends_with("/price-changed"));
        assert_eq!(body["code"], "price_changed");
        assert_eq!(body["quoted_rate_card"], "rate_old");
        assert_eq!(body["rate_card"], "rate_new");
    }

    #[tokio::test]
    async fn an_invalid_state_is_a_typed_conflict() {
        let (status, body) = rendered(CoreError::InvalidState).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["type"],
            "https://spec.chrono-ai.fun/cma/1.0/problems/invalid-state"
        );
        assert_eq!(body["code"], "invalid_state");
    }

    #[tokio::test]
    async fn price_card_fields_are_absent_from_every_other_problem() {
        for error in every_variant() {
            if matches!(error, CoreError::PriceChanged { .. }) {
                continue;
            }
            let (_, body) = rendered(error).await;
            for field in ["quoted_rate_card", "rate_card"] {
                assert!(body.get(field).is_none(), "unexpected {field} in {body}");
            }
        }
    }

    #[tokio::test]
    async fn bound_fields_are_absent_from_every_other_problem() {
        for error in every_variant() {
            if matches!(error, CoreError::SizeOutOfBounds(_)) {
                continue;
            }
            let (_, body) = rendered(error).await;
            for field in ["dimension", "requested", "minimum", "maximum"] {
                assert!(body.get(field).is_none(), "unexpected {field} in {body}");
            }
        }
    }

    #[tokio::test]
    async fn a_session_limit_is_a_conflict_with_both_counts() {
        let (status, body) = rendered(CoreError::SessionLimitReached { limit: 3, live: 4 }).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["type"]
            .as_str()
            .unwrap()
            .ends_with("/session-limit-reached"));
        assert_eq!(body["code"], "session_limit_reached");
        assert_eq!(body["limit"], 3);
        assert_eq!(body["live"], 4);
    }

    #[tokio::test]
    async fn deployment_capacity_is_a_503_with_all_four_figures_and_never_a_404() {
        let (status, body) = rendered(CoreError::DeploymentAtCapacity {
            requested_cpu_millis: 2_000,
            requested_memory_mib: 4_096,
            free_cpu_millis: 750,
            free_memory_mib: 1_536,
        })
        .await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_ne!(status, StatusCode::NOT_FOUND);
        assert!(body["type"]
            .as_str()
            .unwrap()
            .ends_with("/deployment-at-capacity"));
        assert_eq!(body["code"], "deployment_at_capacity");
        assert_eq!(body["requested_cpu_millis"], 2_000);
        assert_eq!(body["requested_memory_mib"], 4_096);
        assert_eq!(body["free_cpu_millis"], 750);
        assert_eq!(body["free_memory_mib"], 1_536);
    }

    #[test]
    fn a_problem_document_uses_its_registered_media_type() {
        let response =
            Problem(CoreError::SessionLimitReached { limit: 1, live: 1 }).into_response();

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/problem+json"))
        );
    }

    #[tokio::test]
    async fn limit_fields_are_absent_from_every_other_problem() {
        for error in every_variant() {
            if matches!(error, CoreError::SessionLimitReached { .. }) {
                continue;
            }
            let (_, body) = rendered(error).await;
            for field in ["limit", "live"] {
                assert!(body.get(field).is_none(), "unexpected {field} in {body}");
            }
        }
    }

    #[tokio::test]
    async fn deployment_capacity_fields_are_absent_from_every_other_problem() {
        for error in every_variant() {
            if matches!(error, CoreError::DeploymentAtCapacity { .. }) {
                continue;
            }
            let (_, body) = rendered(error).await;
            for field in [
                "requested_cpu_millis",
                "requested_memory_mib",
                "free_cpu_millis",
                "free_memory_mib",
            ] {
                assert!(body.get(field).is_none(), "unexpected {field} in {body}");
            }
        }
    }
}
