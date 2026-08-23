//! RFC 9457 projection of the core's error vocabulary.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::CoreError;

use super::PROBLEM_BASE;

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

        let mut body = problem_document(status, slug, &rendered.to_string(), rendered.code());
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
        let mut response = (status, Json(body)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
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
    serde_json::json!({
        "type": format!("{PROBLEM_BASE}/{slug}"),
        "title": slug.replace('-', " "),
        "status": status.as_u16(),
        "detail": detail,
        "code": code,
    })
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
            CoreError::CapabilityUnsupported("terminal".into()),
            CoreError::InvalidRequest("x".into()),
            CoreError::ServiceNotGranted("service".into()),
            CoreError::ServiceCatalogUnavailable("down".into()),
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
                | CoreError::CapabilityUnsupported(_)
                | CoreError::InvalidRequest(_)
                | CoreError::ServiceNotGranted(_)
                | CoreError::ServiceCatalogUnavailable(_)
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
