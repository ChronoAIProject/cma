//! Route-level profile contract tests.
//!
//! These exercise the JSON boundary rather than calling the handlers directly:
//! the regression was a combination of what one route rendered, what another
//! route accepted, and how Axum rejected a body before either handler ran.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    AgentConfiguration, Authorization, Baseline, Capability, CredentialMode, EnvironmentProfile,
    EnvironmentProfileId, EnvironmentProfileService, EnvironmentProfileStore, EnvironmentVariable,
    KernelId, Page, PageRequest, PermissionMode, ReasoningEffort, ServiceId, SessionMode,
    SessionSize, SkillId, SoftwarePackage, StoreError, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::router;
use crate::{
    auth::AuthenticatedSubject,
    rest::environments_shapes::{
        AgentBody, NyxIdBody, PackageBody, ProfileRequest, SizeBody, VariableBody,
    },
};

const SUBJECT: &str = "subject-profile-round-trip";
const PROFILE_ID: &str = "envp_existing";
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

#[derive(Default)]
struct RecordingProfileStore {
    profiles: Mutex<HashMap<String, EnvironmentProfile>>,
    upserts: AtomicUsize,
}

impl RecordingProfileStore {
    fn seed(&self, profile: EnvironmentProfile) {
        self.profiles
            .lock()
            .expect("profile store lock")
            .insert(profile.id.0.clone(), profile);
    }

    fn len(&self) -> usize {
        self.profiles.lock().expect("profile store lock").len()
    }

    fn snapshot(&self, id: &str) -> Option<Value> {
        self.profiles
            .lock()
            .expect("profile store lock")
            .get(id)
            .map(|profile| serde_json::to_value(profile).expect("stored profiles serialize"))
    }
}

#[async_trait::async_trait]
impl EnvironmentProfileStore for RecordingProfileStore {
    async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError> {
        self.upserts.fetch_add(1, Ordering::SeqCst);
        self.profiles
            .lock()
            .expect("profile store lock")
            .insert(profile.id.0.clone(), profile.clone());
        Ok(())
    }

    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(self
            .profiles
            .lock()
            .expect("profile store lock")
            .get(&id.0)
            .cloned())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        let mut profiles: Vec<_> = self
            .profiles
            .lock()
            .expect("profile store lock")
            .values()
            .filter(|profile| &profile.owner == owner)
            .cloned()
            .collect();
        profiles.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.0.cmp(&left.id.0))
        });
        profiles.truncate(page.limit() as usize);
        Ok(Page {
            items: profiles,
            next: None,
        })
    }

    async fn set_preferred(
        &self,
        owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        let mut profiles = self.profiles.lock().expect("profile store lock");
        if let Some(id) = chosen {
            let owned = profiles
                .get(&id.0)
                .is_some_and(|profile| &profile.owner == owner);
            if !owned {
                return Err(StoreError::Malformed(
                    "selected environment profile is not owned by the subject".into(),
                ));
            }
        }
        for profile in profiles
            .values_mut()
            .filter(|profile| &profile.owner == owner)
        {
            profile.preferred = chosen.is_some_and(|id| profile.id == *id);
        }
        Ok(())
    }

    async fn preferred_for(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(self
            .profiles
            .lock()
            .expect("profile store lock")
            .values()
            .filter(|profile| &profile.owner == owner && profile.preferred)
            .max_by(|left, right| {
                left.updated_at
                    .cmp(&right.updated_at)
                    .then_with(|| left.id.0.cmp(&right.id.0))
            })
            .cloned())
    }

    async fn delete(&self, id: &EnvironmentProfileId) -> Result<(), StoreError> {
        self.profiles
            .lock()
            .expect("profile store lock")
            .remove(&id.0);
        Ok(())
    }
}

struct Harness {
    app: Router,
    store: Arc<RecordingProfileStore>,
}

impl Harness {
    fn new() -> Self {
        let store = Arc::new(RecordingProfileStore::default());
        let service = EnvironmentProfileService::new(
            Arc::clone(&store) as Arc<dyn EnvironmentProfileStore>,
            Baseline::new(
                [SoftwarePackage::new("platform-cli")],
                [SkillId::new("mandatory-skill")],
                [ServiceId::new("mandatory-service")],
            ),
            None,
            None,
            AgentConfiguration {
                kernel: KernelId::new("deployment-kernel"),
                ..Default::default()
            },
            Arc::new(layer2_core::PlatformSettingsService::unconfigured()),
        );
        Self {
            app: router(service),
            store,
        }
    }
}

struct WireResponse {
    status: StatusCode,
    content_type: Option<String>,
    bytes: Vec<u8>,
    json: Value,
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<Value>) -> WireResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&json).expect("request JSON serializes"))
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).expect("request builds");
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId(SUBJECT.into())));
    request
        .extensions_mut()
        .insert(Authorization::from_capabilities([
            Capability::ManageOwnEnvironmentProfiles,
        ]));

    let response = app.clone().oneshot(request).await.expect("route responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body is readable")
        .to_vec();
    let json = serde_json::from_slice(&bytes).expect("profile routes return JSON");
    WireResponse {
        status,
        content_type,
        bytes,
        json,
    }
}

/// Every writable profile field, with every optional choice set away from its
/// default. Struct literals make additions fail to compile until this fixture
/// is updated; comparing its JSON to the GET projection below proves each field
/// also crossed the response boundary.
fn field_complete_request() -> ProfileRequest {
    ProfileRequest {
        name: "field-complete profile".into(),
        variables: vec![VariableBody {
            key: "PROFILE_MODE".into(),
            value: "round-trip".into(),
        }],
        packages: vec![
            PackageBody {
                name: "platform-cli".into(),
                version: Some("2.4.1".into()),
            },
            PackageBody {
                name: "ripgrep".into(),
                version: Some("14.1.1".into()),
            },
        ],
        agent: AgentBody {
            kernel: Some("profile-kernel".into()),
            mode: Some(SessionMode::Conversation),
            model: Some("model-for-round-trip".into()),
            reasoning_effort: Some(ReasoningEffort::XHigh),
            permission: Some(PermissionMode::Unrestricted),
            credential: Some(CredentialMode::Own),
            size: Some(SizeBody {
                cpu_millis: 1_500,
                memory_mib: 3_072,
            }),
            record_tool_bodies: Some(true),
            skills: vec!["mandatory-skill".into(), "chosen-skill".into()],
        },
        nyxid: NyxIdBody {
            services: vec!["mandatory-service".into(), "calendar".into()],
        },
        agent_notes: "Keep the caller's standing instructions byte for byte.".into(),
    }
}

fn writable_projection(view: &Value) -> Value {
    json!({
        "name": view["name"],
        "variables": view["variables"],
        "packages": view["packages"],
        "agent": view["agent"],
        "nyxid": view["nyxid"],
        "agent_notes": view["agent_notes"],
    })
}

fn seeded_profile() -> EnvironmentProfile {
    EnvironmentProfile {
        id: EnvironmentProfileId(PROFILE_ID.into()),
        owner: SubjectId(SUBJECT.into()),
        name: "unchanged profile".into(),
        variables: vec![EnvironmentVariable::new("KEEP", "this-value")],
        packages: vec![SoftwarePackage::new("platform-cli")],
        agent: AgentConfiguration {
            kernel: KernelId::new("profile-kernel"),
            mode: SessionMode::Conversation,
            model: Some("keep-model".into()),
            reasoning_effort: ReasoningEffort::XHigh,
            permission: PermissionMode::Unrestricted,
            credential: CredentialMode::Own,
            size: Some(SessionSize::new(1_500, 3_072).expect("valid session size")),
            record_tool_bodies: true,
            skills: vec![SkillId::new("mandatory-skill")],
        },
        services: vec![ServiceId::new("mandatory-service")],
        agent_notes: "keep these notes".into(),
        created_at: "2026-08-25T00:00:00Z".into(),
        updated_at: "2026-08-25T01:00:00Z".into(),
        preferred: false,
    }
}

/// Profile timestamps have one-second resolution. Start the exact-byte test
/// near the beginning of a clock second so its intentional echoing PUT cannot
/// acquire a different `updated_at` merely by crossing a wall-clock boundary.
async fn leave_clock_boundary() {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis();
    if millis > 100 {
        tokio::time::sleep(Duration::from_millis(u64::from(1_050 - millis))).await;
    }
}

#[tokio::test]
async fn a_field_complete_get_put_get_is_byte_identical() {
    leave_clock_boundary().await;
    let harness = Harness::new();
    let submitted = serde_json::to_value(field_complete_request()).expect("request serializes");

    let created = send(
        &harness.app,
        Method::POST,
        "/api/v1/environment-profiles",
        Some(submitted.clone()),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.json["agent"]["record_tool_bodies"], true);
    let id = created.json["id"]
        .as_str()
        .expect("created profile has an id")
        .to_owned();

    let profile_uri = format!("/api/v1/environment-profiles/{id}");
    let before = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(before.json["agent"]["record_tool_bodies"], true);
    assert_eq!(before.json["nyxid"]["services"][1], "calendar");
    assert_eq!(
        before.json["agent_notes"],
        "Keep the caller's standing instructions byte for byte."
    );
    assert_eq!(
        before.json["mandatory"],
        json!({
            "packages": ["platform-cli"],
            "skills": ["mandatory-skill"],
        })
    );
    assert_eq!(before.json["preferred"], false);

    let echoed = writable_projection(&before.json);
    assert_eq!(
        echoed, submitted,
        "GET must render every field the write shape accepted"
    );
    let replaced = send(&harness.app, Method::PUT, &profile_uri, Some(echoed)).await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json["agent"]["record_tool_bodies"], true);

    let after = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(after.status, StatusCode::OK);
    assert_eq!(after.json["agent"]["record_tool_bodies"], true);
    assert_eq!(
        after.bytes, before.bytes,
        "echoing every writable field must leave the complete ProfileView JSON byte-identical"
    );
}

#[tokio::test]
async fn replacement_omissions_reset_writable_fields_to_defaults() {
    let harness = Harness::new();
    harness.store.seed(seeded_profile());

    let replaced = send(
        &harness.app,
        Method::PUT,
        &format!("/api/v1/environment-profiles/{PROFILE_ID}"),
        Some(json!({
            "name": "replacement",
            "agent": { "kernel": "profile-kernel" },
        })),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json["name"], "replacement");
    assert_eq!(replaced.json["variables"], json!([]));
    assert_eq!(
        replaced.json["packages"],
        json!([{ "name": "platform-cli" }])
    );
    assert_eq!(replaced.json["agent"]["mode"], "terminal");
    assert!(replaced.json["agent"].get("model").is_none());
    assert_eq!(replaced.json["agent"]["reasoning_effort"], "high");
    assert_eq!(replaced.json["agent"]["permission"], "guarded");
    assert_eq!(replaced.json["agent"]["credential"], "delegated");
    assert!(replaced.json["agent"].get("size").is_none());
    assert_eq!(replaced.json["agent"]["record_tool_bodies"], false);
    assert_eq!(replaced.json["agent"]["skills"], json!(["mandatory-skill"]));
    assert_eq!(
        replaced.json["nyxid"]["services"],
        json!(["mandatory-service"])
    );
    assert_eq!(replaced.json["agent_notes"], "");
}

#[test]
fn every_nested_profile_shape_identifies_an_unknown_field() {
    for (unknown, request) in [
        (
            "unexpected_profile_field",
            json!({ "name": "profile", "unexpected_profile_field": true }),
        ),
        (
            "variableValue",
            json!({
                "name": "profile",
                "variables": [{ "key": "A", "value": "1", "variableValue": "1" }],
            }),
        ),
        (
            "packageVersion",
            json!({
                "name": "profile",
                "packages": [{ "name": "ripgrep", "packageVersion": "14" }],
            }),
        ),
        (
            "reasoningEffort",
            json!({ "name": "profile", "agent": { "reasoningEffort": "low" } }),
        ),
        (
            "memoryMb",
            json!({
                "name": "profile",
                "agent": {
                    "size": { "cpu_millis": 500, "memory_mib": 512, "memoryMb": 512 },
                },
            }),
        ),
        (
            "serviceIds",
            json!({ "name": "profile", "nyxid": { "serviceIds": ["calendar"] } }),
        ),
    ] {
        let message = serde_json::from_value::<ProfileRequest>(request)
            .expect_err("an unknown field must be refused")
            .to_string();
        assert!(
            message.contains(unknown),
            "schema refusal must identify {unknown}, got {message}"
        );
    }
}

#[tokio::test]
async fn create_and_replace_unknown_fields_are_confidential_problem_documents() {
    for (unknown, submitted_value, body) in [
        (
            "unexpected_profile_field",
            "do-not-reflect-top-level",
            json!({
                "name": "profile",
                "unexpected_profile_field": "do-not-reflect-top-level",
            }),
        ),
        (
            "reasoningEffort",
            "do-not-reflect-nested",
            json!({
                "name": "profile",
                "agent": { "reasoningEffort": "do-not-reflect-nested" },
            }),
        ),
    ] {
        for method in [Method::POST, Method::PUT] {
            let harness = Harness::new();
            let uri = if method == Method::POST {
                "/api/v1/environment-profiles".to_owned()
            } else {
                harness.store.seed(seeded_profile());
                format!("/api/v1/environment-profiles/{PROFILE_ID}")
            };
            let before = harness.store.snapshot(PROFILE_ID);

            let refused = send(&harness.app, method.clone(), &uri, Some(body.clone())).await;
            assert_eq!(
                refused.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{method} must refuse {unknown}"
            );
            assert_eq!(refused.content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
            assert_eq!(refused.json["status"], 422);
            assert_eq!(refused.json["code"], "invalid_request");
            assert_eq!(
                refused.json["detail"],
                "The JSON body does not match this operation's request schema."
            );
            let rendered = String::from_utf8(refused.bytes).expect("problem JSON is UTF-8");
            assert!(!rendered.contains(unknown), "unknown keys are request data");
            assert!(
                !rendered.contains(submitted_value),
                "a rejected value must not be reflected"
            );
            assert_eq!(harness.store.upserts.load(Ordering::SeqCst), 0);
            if method == Method::POST {
                assert_eq!(harness.store.len(), 0, "a refused create stores nothing");
            } else {
                assert_eq!(
                    harness.store.snapshot(PROFILE_ID),
                    before,
                    "a refused replacement leaves the stored profile untouched"
                );
            }
        }
    }
}
