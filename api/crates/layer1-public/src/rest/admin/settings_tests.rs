//! End-to-end tests for the operator settings routes.

use std::sync::Mutex;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use layer2_core::{Authorization, Capability, PlatformSettingsStore, StoreError, SubjectId};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::auth::AuthenticatedSubject;

#[derive(Default)]
struct FakeStore {
    held: Mutex<Option<StoredSettings>>,
}

#[async_trait::async_trait]
impl PlatformSettingsStore for FakeStore {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Ok(self.held.lock().expect("not poisoned").clone())
    }

    async fn save_if_unchanged(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        let mut held = self.held.lock().expect("not poisoned");
        let revision = held.as_ref().map_or(0, |stored| stored.revision);
        if revision != expected_revision {
            return Ok(None);
        }
        let next = StoredSettings {
            settings: settings.clone(),
            revision: revision + 1,
            previous: held.as_ref().map(|stored| stored.settings.clone()),
        };
        *held = Some(next.clone());
        Ok(Some(next))
    }
}

fn app() -> Router {
    router(Arc::new(PlatformSettingsService::new(Arc::new(
        FakeStore::default(),
    ))))
}

fn request(method: Method, path: &str, body: Option<Value>, admin: bool) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).expect("request builds");
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId("operator".into())));
    request.extensions_mut().insert(if admin {
        Authorization::from_capabilities([Capability::ConfigurePlatform])
    } else {
        Authorization::none()
    });
    request
}

async fn response_json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    serde_json::from_slice(&bytes).expect("response is json")
}

fn priced_settings() -> (PlatformSettings, String) {
    let mut settings = PlatformSettings::default();
    settings.pricing = settings.pricing.appended(
        layer2_core::Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        "2026-08-23T01:00:00Z",
    );
    let card = settings
        .pricing
        .in_force()
        .expect("the appended card is in force")
        .id
        .0
        .clone();
    (settings, card)
}

fn changed_briefing() -> Value {
    json!({
        "before": ["# Changed", "Platform prose"],
        "reach_heading": "## Granted services",
        "reach_lead": "Use only these services:",
        "reach_close": "Other services remain unavailable.",
        "reach_none": "No services were granted.",
        "after": ["End of platform prose."]
    })
}

#[tokio::test]
async fn an_administrator_can_read_change_and_reset_the_briefing() {
    let app = app();
    let initial = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/admin/settings", None, true))
        .await
        .expect("route answers");
    assert_eq!(initial.status(), StatusCode::OK);
    let initial = response_json(initial).await;
    assert_eq!(initial["revision"], 0);
    assert!(initial.get("previous").is_none());

    let changed = app
        .clone()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "briefing": changed_briefing(),
                "size_bounds": {
                    "min_cpu_millis": 500,
                    "max_cpu_millis": 3000,
                    "min_memory_mib": 512,
                    "max_memory_mib": 4096
                },
                "live_sessions": 2
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = response_json(changed).await;
    assert_eq!(changed["revision"], 1);
    assert_eq!(changed["settings"]["briefing"], changed_briefing());

    let reset = app
        .oneshot(request(
            Method::POST,
            "/api/v1/admin/settings/reset",
            Some(json!({ "expected_revision": 1 })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(reset.status(), StatusCode::OK);
    let reset = response_json(reset).await;
    assert_eq!(reset["revision"], 2);
    assert_eq!(
        reset["settings"]["size_bounds"],
        changed["settings"]["size_bounds"]
    );
    assert_eq!(reset["settings"]["live_sessions"], 2);
    assert_ne!(reset["settings"]["briefing"], changed_briefing());
    assert_eq!(reset["previous"], changed["settings"]);
}

#[tokio::test]
async fn a_complete_save_reports_the_settings_and_card_that_actually_changed() {
    let (mut settings, rate_card) = priced_settings();
    settings
        .briefing
        .before
        .push("Changed with the price".into());
    settings.deployment_capacity = layer2_core::DeploymentCapacity {
        cpu_millis: Some(8_000),
        memory_mib: Some(16_384),
        assumed_session_size: Some(layer2_core::SessionSize::new(1_000, 1_024).unwrap()),
    };
    let response = app()
        .oneshot(request(
            Method::PUT,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "settings": settings
            })),
            true,
        ))
        .await
        .expect("route answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.extensions().get::<SettingsTouched>(),
        Some(&SettingsTouched {
            changed: vec!["briefing", "deployment_capacity", "pricing"],
            rate_card: Some(rate_card.clone()),
        })
    );
    let body = response_json(response).await;
    assert_eq!(body["revision"], 1);
    assert_eq!(body["settings"]["pricing"]["cards"][0]["id"], rate_card);
    assert_eq!(body["settings"]["deployment_capacity"]["cpu_millis"], 8_000);
}

#[tokio::test]
async fn deployment_capacity_can_be_conditionally_replaced_and_then_preserved() {
    let app = app();
    let changed = app
        .clone()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "deployment_capacity": {
                    "cpu_millis": 20_000,
                    "memory_mib": 40_960,
                    "assumed_session_size": {
                        "cpu_millis": 1_000,
                        "memory_mib": 1_024
                    }
                }
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = response_json(changed).await;
    assert_eq!(
        changed["settings"]["deployment_capacity"],
        json!({
            "cpu_millis": 20_000,
            "memory_mib": 40_960,
            "assumed_session_size": {
                "cpu_millis": 1_000,
                "memory_mib": 1_024
            }
        })
    );

    let preserved = app
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 1, "live_sessions": 3 })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(preserved.status(), StatusCode::OK);
    assert_eq!(
        response_json(preserved).await["settings"]["deployment_capacity"],
        changed["settings"]["deployment_capacity"]
    );
}

#[tokio::test]
async fn an_unconfigured_service_reads_defaults_and_refuses_a_save_by_name() {
    let app = router(Arc::new(PlatformSettingsService::unconfigured()));
    let read = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/admin/settings", None, true))
        .await
        .expect("route answers");
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(response_json(read).await["revision"], 0);

    let save = app
        .oneshot(request(
            Method::PUT,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "settings": PlatformSettings::default()
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(save.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response_json(save).await["code"], "settings_unavailable");
}

#[tokio::test]
async fn a_complete_save_refuses_fields_outside_its_document() {
    let response = app()
        .oneshot(request(
            Method::PUT,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "settings": PlatformSettings::default(),
                "briefing": "not part of this request shape"
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response_json(response).await["code"], "invalid_request");
}

#[tokio::test]
async fn omission_preserves_a_setting_and_null_only_removes_the_optional_limit() {
    let app = app();
    let first = app
        .clone()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "briefing": changed_briefing(),
                "live_sessions": 4
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 1, "live_sessions": null })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(second.status(), StatusCode::OK);
    let second = response_json(second).await;
    assert_eq!(second["settings"]["briefing"], changed_briefing());
    assert!(second["settings"]["live_sessions"].is_null());
    assert_eq!(second["previous"]["live_sessions"], 4);
}

#[tokio::test]
async fn empty_null_unknown_and_stale_changes_are_refused_by_name() {
    let app = app();
    for (body, status, code) in [
        (
            json!({ "expected_revision": 0 }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
        ),
        (
            json!({ "expected_revision": 0, "briefing": null }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
        ),
        (
            json!({ "expected_revision": 0, "deployment_capacity": null }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
        ),
        (
            json!({ "expected_revision": 0, "invented": true }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                Method::PATCH,
                "/api/v1/admin/settings",
                Some(body),
                true,
            ))
            .await
            .expect("route answers");
        assert_eq!(response.status(), status);
        assert_eq!(response_json(response).await["code"], code);
    }

    let written = app
        .clone()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 0, "live_sessions": 2 })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(written.status(), StatusCode::OK);
    let stale = app
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 0, "live_sessions": 3 })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(stale).await["code"], "settings_conflict");
}

#[tokio::test]
async fn a_body_error_never_reflects_administrator_prose() {
    let planted = "private administrator wording that must not return";
    let response = app()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "briefing": {
                    "before": planted,
                    "reach_heading": "heading",
                    "reach_lead": "lead",
                    "reach_close": "close",
                    "reach_none": "none",
                    "after": []
                }
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body = response_json(response).await;
    assert_eq!(body["code"], "invalid_request");
    assert!(!body.to_string().contains(planted));
}

#[tokio::test]
async fn a_non_administrator_gets_the_operator_refusal() {
    let response = app()
        .oneshot(request(Method::GET, "/api/v1/admin/settings", None, false))
        .await
        .expect("route answers");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response_json(response).await;
    assert!(body["type"]
        .as_str()
        .expect("type is a string")
        .ends_with("/not-an-operator"));
}
