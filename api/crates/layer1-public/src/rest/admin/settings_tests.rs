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

fn app() -> axum::Router {
    router(Arc::new(PlatformSettingsService::new(Arc::new(
        FakeStore::default(),
    ))))
    .into()
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

fn complete_chat_settings() -> Value {
    json!({
        "enabled": true,
        "workspace": {
            "repository_url": "https://example.test/team/chat-workspace.git",
            "branch": "development"
        },
        "agent": {
            "model": "reasoning-model",
            "reasoning_effort": "x_high",
            "instructions": "Follow the repository's agent guidance.",
            "record_tool_bodies": false
        },
        "message_prefixes": ["/formalize", "/check"],
        "inactivity": { "pause_after_seconds": 1200 },
        "setup": { "commands": ["tool install", "tool verify"] },
        "host": {
            "size": { "cpu_millis": 2000, "memory_mib": 4096 },
            "max_threads": 12
        }
    })
}

fn submitted_texts(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::String(text) if text.len() >= 4 => texts.push(text.clone()),
        Value::Array(values) => {
            for value in values {
                submitted_texts(value, texts);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                submitted_texts(value, texts);
            }
        }
        _ => {}
    }
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
async fn a_full_chat_object_round_trips_and_a_chat_only_patch_preserves_everything_else() {
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

    let changed = app
        .clone()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 1,
                "chat": complete_chat_settings()
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(
        changed.extensions().get::<SettingsTouched>(),
        Some(&SettingsTouched {
            changed: vec!["chat"],
            rate_card: None,
        })
    );
    let changed = response_json(changed).await;
    assert_eq!(changed["revision"], 2);
    assert_eq!(changed["settings"]["chat"], complete_chat_settings());
    assert_eq!(changed["settings"]["briefing"], changed_briefing());
    assert_eq!(changed["settings"]["live_sessions"], 4);

    let read = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/admin/settings", None, true))
        .await
        .expect("route answers");
    assert_eq!(response_json(read).await["settings"], changed["settings"]);

    let reset = app
        .oneshot(request(
            Method::POST,
            "/api/v1/admin/settings/reset",
            Some(json!({ "expected_revision": 2 })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(reset.status(), StatusCode::OK);
    let reset = response_json(reset).await;
    assert!(reset["settings"]["chat"].is_null());
    assert_eq!(reset["settings"]["live_sessions"], 4);
}

#[tokio::test]
async fn omitted_chat_fields_take_the_documented_defaults() {
    let response = app()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 0, "chat": {} })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(response.status(), StatusCode::OK);
    let chat = response_json(response).await["settings"]["chat"].clone();
    assert_eq!(chat["enabled"], false);
    assert_eq!(
        chat["workspace"]["repository_url"],
        "https://github.com/the-omega-institute/trureturing"
    );
    assert_eq!(chat["workspace"]["branch"], "dev");
    assert_eq!(chat["agent"]["reasoning_effort"], "high");
    assert_eq!(chat["agent"]["record_tool_bodies"], true);
    assert_eq!(chat["message_prefixes"], json!(["/codex-formalize"]));
    assert_eq!(chat["inactivity"]["pause_after_seconds"], 900);
    assert_eq!(chat["host"]["max_threads"], 10);
}

#[tokio::test]
async fn exact_chat_boundaries_are_inclusive_in_utf8_bytes() {
    let repository_start = "https://example.test/";
    let repository_end = "/repo";
    let repository_url = format!(
        "{repository_start}{}{repository_end}",
        "a".repeat(512 - repository_start.len() - repository_end.len())
    );
    assert_eq!(repository_url.len(), 512);

    let mut upper = complete_chat_settings();
    upper["workspace"]["repository_url"] = repository_url.into();
    upper["workspace"]["branch"] = "é".repeat(100).into();
    upper["message_prefixes"] = json!(vec!["💡".repeat(64); 8]);
    upper["inactivity"]["pause_after_seconds"] = 86_400.into();
    upper["setup"]["commands"] = json!(vec!["💡".repeat(1_024); 16]);
    upper["host"]["size"]["cpu_millis"] = u32::MAX.into();
    upper["host"]["size"]["memory_mib"] = u32::MAX.into();
    upper["host"]["max_threads"] = 50.into();

    let app = app();
    let response = app
        .clone()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 0, "chat": upper })),
            true,
        ))
        .await
        .expect("upper boundaries answer");
    assert_eq!(response.status(), StatusCode::OK);

    let lower = json!({
        "workspace": {
            "repository_url": "https://x.co/r",
            "branch": "b"
        },
        "message_prefixes": [],
        "inactivity": { "pause_after_seconds": 60 },
        "setup": { "commands": [] },
        "host": {
            "size": { "cpu_millis": 1, "memory_mib": 1 },
            "max_threads": 1
        }
    });
    let response = app
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({ "expected_revision": 1, "chat": lower })),
            true,
        ))
        .await
        .expect("lower boundaries answer");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn every_chat_validation_boundary_is_a_named_confidential_refusal() {
    let long_url = format!("https://example.test/{}", "r".repeat(500));
    let long_branch = "b".repeat(201);
    let long_prefix = "p".repeat(257);
    let long_command = "c".repeat(4_097);
    let multibyte_branch = "é".repeat(101);
    let multibyte_prefix = "💡".repeat(65);
    let multibyte_command = "💡".repeat(1_025);
    let cases = vec![
        (
            json!({"workspace": {"repository_url": "http://example.test/repo"}}),
            "repository_url",
            "chat.workspace.repository_url",
        ),
        (
            json!({"workspace": {"repository_url": "https://user@example.test/repo"}}),
            "repository_url",
            "chat.workspace.repository_url",
        ),
        (
            json!({"workspace": {"repository_url": long_url}}),
            "repository_url",
            "chat.workspace.repository_url",
        ),
        (
            json!({"workspace": {"branch": ""}}),
            "branch",
            "chat.workspace.branch",
        ),
        (
            json!({"workspace": {"branch": long_branch}}),
            "branch",
            "chat.workspace.branch",
        ),
        (
            json!({"workspace": {"branch": multibyte_branch}}),
            "branch",
            "chat.workspace.branch",
        ),
        (
            json!({"workspace": {"branch": "two words"}}),
            "branch",
            "chat.workspace.branch",
        ),
        (
            json!({"message_prefixes": ["p", "p", "p", "p", "p", "p", "p", "p", "p"]}),
            "message_prefixes",
            "chat.message_prefixes",
        ),
        (
            json!({"message_prefixes": ["  "]}),
            "message_prefixes",
            "chat.message_prefixes[0]",
        ),
        (
            json!({"message_prefixes": [long_prefix]}),
            "message_prefixes",
            "chat.message_prefixes[0]",
        ),
        (
            json!({"message_prefixes": [multibyte_prefix]}),
            "message_prefixes",
            "chat.message_prefixes[0]",
        ),
        (
            json!({"message_prefixes": ["first\nsecond"]}),
            "message_prefixes",
            "chat.message_prefixes[0]",
        ),
        (
            json!({"inactivity": {"pause_after_seconds": 59}}),
            "pause_after_seconds",
            "chat.inactivity.pause_after_seconds",
        ),
        (
            json!({"inactivity": {"pause_after_seconds": 86_401}}),
            "pause_after_seconds",
            "chat.inactivity.pause_after_seconds",
        ),
        (
            json!({"setup": {"commands": vec![""; 17]}}),
            "commands",
            "chat.setup.commands",
        ),
        (
            json!({"setup": {"commands": [long_command]}}),
            "commands",
            "chat.setup.commands[0]",
        ),
        (
            json!({"setup": {"commands": [multibyte_command]}}),
            "commands",
            "chat.setup.commands[0]",
        ),
        (
            json!({"host": {"size": {"cpu_millis": 0, "memory_mib": 512}}}),
            "size",
            "chat.host.size",
        ),
        (
            json!({"host": {"size": {"cpu_millis": 500, "memory_mib": 0}}}),
            "size",
            "chat.host.size",
        ),
        (
            json!({"host": {"max_threads": 0}}),
            "max_threads",
            "chat.host.max_threads",
        ),
        (
            json!({"host": {"max_threads": 51}}),
            "max_threads",
            "chat.host.max_threads",
        ),
    ];

    for (chat, field, path) in cases {
        let submitted = chat.to_string();
        let mut scalar_texts = Vec::new();
        submitted_texts(&chat, &mut scalar_texts);
        let response = app()
            .oneshot(request(
                Method::PATCH,
                "/api/v1/admin/settings",
                Some(json!({ "expected_revision": 0, "chat": chat })),
                true,
            ))
            .await
            .expect("route answers");
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}"
        );
        let body = response_json(response).await;
        assert_eq!(body["code"], "invalid_request", "{path}");
        assert_eq!(body["field"], field, "{path}");
        assert_eq!(body["path"], path, "{path}");
        assert!(
            !body.to_string().contains(&submitted),
            "submitted object leaked for {path}"
        );
        let rendered = body.to_string();
        for scalar in scalar_texts {
            assert!(
                !rendered.contains(&scalar),
                "submitted scalar leaked for {path}"
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_nested_chat_field_is_named_without_its_value() {
    let planted = "private-value-must-not-return";
    let response = app()
        .oneshot(request(
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "chat": { "agent": { "secret_model_token": planted } }
            })),
            true,
        ))
        .await
        .expect("route answers");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_json(response).await;
    assert_eq!(body["field"], "secret_model_token");
    assert_eq!(body["path"], "chat.agent.secret_model_token");
    assert!(!body.to_string().contains(planted));
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
    let app: axum::Router = router(Arc::new(PlatformSettingsService::unconfigured())).into();
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
