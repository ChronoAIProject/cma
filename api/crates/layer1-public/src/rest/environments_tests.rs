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
};

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    AgentConfiguration, Authorization, Baseline, Capability, CatalogError, CatalogService,
    CatalogSkill, CredentialMode, EnvironmentProfile, EnvironmentProfileId,
    EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentProfileStore,
    EnvironmentVariable, KernelId, Page, PageRequest, PermissionMode, ReasoningEffort,
    ServiceCatalog, ServiceId, SessionMode, SessionSize, SkillCatalog, SkillId, SoftwarePackage,
    StoreError, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::router;
use crate::{
    auth::AuthenticatedSubject,
    rest::environments_shapes::{
        decode_profile_request, unknown_profile_field, AgentBody, NyxIdBody, PackageBody,
        ProfileRequest, ProfileView, SizeBody, VariableBody,
    },
};

const SUBJECT: &str = "subject-profile-round-trip";
const PROFILE_ID: &str = "envp_existing";
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";
const API_REST_DOCUMENTATION: &str = include_str!("../../../../../docs/API_REST.md");

struct DocumentationCatalog;

#[async_trait::async_trait]
impl SkillCatalog for DocumentationCatalog {
    async fn available(
        &self,
        _subject: &SubjectId,
        _credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        Ok(vec![CatalogSkill {
            id: SkillId::new("chosen-skill"),
            name: "Repository review".into(),
            description: "Review a repository without changing it".into(),
            access: Some("owner".into()),
        }])
    }
}

#[async_trait::async_trait]
impl ServiceCatalog for DocumentationCatalog {
    async fn available(
        &self,
        _subject: &SubjectId,
        _credential: Option<&str>,
    ) -> Result<Vec<CatalogService>, CatalogError> {
        Ok(vec![CatalogService {
            id: ServiceId::new("calendar"),
            name: "Calendar".into(),
            description: Some("Read and manage calendar events".into()),
        }])
    }
}

struct UnavailableCatalog;

#[async_trait::async_trait]
impl SkillCatalog for UnavailableCatalog {
    async fn available(
        &self,
        _subject: &SubjectId,
        _credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        Err(CatalogError::Unavailable("test skill outage".into()))
    }
}

#[async_trait::async_trait]
impl ServiceCatalog for UnavailableCatalog {
    async fn available(
        &self,
        _subject: &SubjectId,
        _credential: Option<&str>,
    ) -> Result<Vec<CatalogService>, CatalogError> {
        Err(CatalogError::Unavailable("test service outage".into()))
    }
}

#[derive(Default)]
struct RecordingProfileStore {
    profiles: Mutex<HashMap<String, EnvironmentProfile>>,
    upserts: AtomicUsize,
    replacements: AtomicUsize,
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

    async fn replace_content(
        &self,
        id: &EnvironmentProfileId,
        content: &EnvironmentProfileSpec,
        updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        let mut profiles = self.profiles.lock().expect("profile store lock");
        let Some(profile) = profiles.get_mut(&id.0) else {
            return Ok(None);
        };
        profile.apply_content(content, updated_at);
        let profile = profile.clone();
        self.replacements.fetch_add(1, Ordering::SeqCst);
        Ok(Some(profile))
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
    service: Arc<EnvironmentProfileService>,
}

impl Harness {
    fn new() -> Self {
        Self::with_baseline(Self::default_baseline())
    }

    fn default_baseline() -> Baseline {
        Baseline::new(
            [SoftwarePackage::new("platform-cli")],
            [SkillId::new("mandatory-skill")],
            [ServiceId::new("mandatory-service")],
        )
    }

    fn with_baseline(baseline: Baseline) -> Self {
        let catalog = Arc::new(DocumentationCatalog);
        let skills: Arc<dyn SkillCatalog> = catalog.clone();
        let services: Arc<dyn ServiceCatalog> = catalog;
        Self::with_catalogs(baseline, Some(skills), Some(services))
    }

    fn with_unavailable_catalogs() -> Self {
        let catalog = Arc::new(UnavailableCatalog);
        let skills: Arc<dyn SkillCatalog> = catalog.clone();
        let services: Arc<dyn ServiceCatalog> = catalog;
        Self::with_catalogs(Self::default_baseline(), Some(skills), Some(services))
    }

    fn with_catalogs(
        baseline: Baseline,
        skills: Option<Arc<dyn SkillCatalog>>,
        services: Option<Arc<dyn ServiceCatalog>>,
    ) -> Self {
        let store = Arc::new(RecordingProfileStore::default());
        let service = EnvironmentProfileService::new(
            Arc::clone(&store) as Arc<dyn EnvironmentProfileStore>,
            baseline,
            skills,
            services,
            AgentConfiguration {
                kernel: KernelId::new("deployment-kernel"),
                ..Default::default()
            },
            Arc::new(layer2_core::PlatformSettingsService::unconfigured()),
        );
        Self {
            app: router(Arc::clone(&service)),
            store,
            service,
        }
    }
}

struct WireResponse {
    status: StatusCode,
    content_type: Option<String>,
    bytes: Vec<u8>,
    json: Value,
}

fn authenticated_request(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
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
    request
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<Value>) -> WireResponse {
    let response = app
        .clone()
        .oneshot(authenticated_request(method, uri, body))
        .await
        .expect("route responds");
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

async fn send_without_json(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(authenticated_request(method, uri, body))
        .await
        .expect("route responds");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body is readable")
        .to_vec();
    (status, bytes)
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

fn legacy_profile() -> EnvironmentProfile {
    let mut profile = seeded_profile();
    profile.name = "legacy profile".into();
    profile.packages.clear();
    profile.agent.kernel = KernelId::default();
    profile.agent.skills.clear();
    profile.services.clear();
    profile.updated_at = "2025-01-01T00:00:00Z".into();
    profile
}

fn retired_baseline_boundary_profile() -> EnvironmentProfile {
    let mut profile = seeded_profile();
    profile.name = "retired baseline boundary".into();
    profile.packages = std::iter::once(SoftwarePackage::new("old-mandatory-package"))
        .chain((0..100).map(|i| SoftwarePackage::new(format!("caller-package-{i}"))))
        .collect();
    profile.agent.skills = std::iter::once(SkillId::new("old-mandatory-skill"))
        .chain((0..100).map(|i| SkillId::new(format!("caller-skill-{i}"))))
        .collect();
    profile.services = std::iter::once(ServiceId::new("old-mandatory-service"))
        .chain((0..100).map(|i| ServiceId::new(format!("caller-service-{i}"))))
        .collect();
    profile.updated_at = "2025-01-01T00:00:00Z".into();
    profile
}

fn documented_profile(preferred: bool) -> EnvironmentProfile {
    EnvironmentProfile {
        id: EnvironmentProfileId("envp_example".into()),
        owner: SubjectId(SUBJECT.into()),
        name: "field-complete profile".into(),
        variables: vec![EnvironmentVariable::new("PROFILE_MODE", "round-trip")],
        packages: vec![
            SoftwarePackage {
                name: "platform-cli".into(),
                version: Some("2.4.1".into()),
            },
            SoftwarePackage {
                name: "ripgrep".into(),
                version: Some("14.1.1".into()),
            },
        ],
        agent: AgentConfiguration {
            kernel: KernelId::new("profile-kernel"),
            mode: SessionMode::Conversation,
            model: Some("model-for-round-trip".into()),
            reasoning_effort: ReasoningEffort::XHigh,
            permission: PermissionMode::Unrestricted,
            credential: CredentialMode::Own,
            size: Some(SessionSize::new(1_500, 3_072).expect("valid session size")),
            record_tool_bodies: true,
            skills: vec![
                SkillId::new("mandatory-skill"),
                SkillId::new("chosen-skill"),
            ],
        },
        services: vec![
            ServiceId::new("mandatory-service"),
            ServiceId::new("calendar"),
        ],
        agent_notes: "Keep the caller's standing instructions byte for byte.".into(),
        created_at: "2026-08-25T09:00:00Z".into(),
        updated_at: "2026-08-25T09:00:00Z".into(),
        preferred,
    }
}

fn environment_profile_documentation() -> &'static str {
    let (_, after_heading) = API_REST_DOCUMENTATION
        .split_once("## 3. Environment Profiles")
        .expect("the environment-profile section exists");
    after_heading
        .split_once("## 4. Sessions")
        .expect("the profile section precedes sessions")
        .0
}

fn marked_examples(section: &str, marker: &str) -> Vec<Value> {
    let opening = format!("<!-- {marker} -->\n```json\n");
    let mut examples = Vec::new();
    let mut rest = section;
    while let Some((_, after_opening)) = rest.split_once(&opening) {
        let (json, after_example) = after_opening
            .split_once("\n```")
            .expect("a marked JSON example has a closing fence");
        examples.push(serde_json::from_str(json).expect("a marked example is valid JSON"));
        rest = after_example;
    }
    assert!(
        !examples.is_empty(),
        "the documentation carries a {marker} example"
    );
    examples
}

#[tokio::test]
async fn a_field_complete_get_put_get_is_byte_identical() {
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

    let stored_before = harness
        .store
        .snapshot(&id)
        .expect("the created profile is stored");
    let echoed = send(
        &harness.app,
        Method::PUT,
        &profile_uri,
        Some(before.json.clone()),
    )
    .await;
    assert_eq!(echoed.status, StatusCode::OK);
    assert_eq!(echoed.bytes, before.bytes);
    assert_eq!(harness.store.upserts.load(Ordering::SeqCst), 1);
    assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);
    assert_eq!(
        harness.store.snapshot(&id),
        Some(stored_before.clone()),
        "a literal ProfileView echo is a no-op with no store mutation"
    );

    let after_raw_echo = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(after_raw_echo.bytes, before.bytes);

    let writable = writable_projection(&before.json);
    assert_eq!(
        writable, submitted,
        "GET must render every field the write shape accepted"
    );
    let replaced = send(&harness.app, Method::PUT, &profile_uri, Some(writable)).await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json["agent"]["record_tool_bodies"], true);
    assert_eq!(
        harness.store.upserts.load(Ordering::SeqCst),
        1,
        "an exact echo is a no-op rather than a timestamp-only store write"
    );
    assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);

    let after = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(after.status, StatusCode::OK);
    assert_eq!(after.json["agent"]["record_tool_bodies"], true);
    assert_eq!(
        after.bytes, before.bytes,
        "echoing every writable field must leave the complete ProfileView JSON byte-identical"
    );
}

#[tokio::test]
async fn a_transformed_profile_view_cannot_overwrite_server_owned_fields() {
    let harness = Harness::new();
    harness.store.seed(seeded_profile());
    let profile_uri = format!("/api/v1/environment-profiles/{PROFILE_ID}");
    let before = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(before.status, StatusCode::OK);

    let mut transformed = before.json.clone();
    transformed["name"] = json!("transformed profile");
    transformed["mandatory"] = json!({
        "packages": ["caller-cannot-replace-the-baseline"],
        "skills": ["caller-cannot-replace-the-baseline"],
    });
    transformed["created_at"] = json!("2099-01-01T00:00:00Z");
    transformed["updated_at"] = json!("2099-01-01T00:00:00Z");
    transformed["preferred"] = json!(true);

    let replaced = send(&harness.app, Method::PUT, &profile_uri, Some(transformed)).await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json["id"], PROFILE_ID);
    assert_eq!(replaced.json["name"], "transformed profile");
    assert_eq!(replaced.json["created_at"], before.json["created_at"]);
    assert_ne!(replaced.json["updated_at"], "2099-01-01T00:00:00Z");
    assert_eq!(replaced.json["preferred"], false);
    assert_eq!(
        replaced.json["mandatory"],
        json!({
            "packages": ["platform-cli"],
            "skills": ["mandatory-skill"],
        })
    );
    assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replacement_refuses_mismatched_or_malformed_profile_view_metadata() {
    for (label, mutate) in [
        ("mismatched id", ("id", json!("envp_somebody_else"))),
        ("non-string timestamp", ("created_at", json!(42))),
        ("non-boolean preference", ("preferred", json!("yes"))),
        ("null server-owned field", ("updated_at", Value::Null)),
        (
            "malformed mandatory shape",
            ("mandatory", json!({ "packages": ["platform-cli"] })),
        ),
    ] {
        let harness = Harness::new();
        harness.store.seed(seeded_profile());
        let before = harness
            .store
            .snapshot(PROFILE_ID)
            .expect("the profile is stored");
        let profile_uri = format!("/api/v1/environment-profiles/{PROFILE_ID}");
        let mut view = send(&harness.app, Method::GET, &profile_uri, None)
            .await
            .json;
        view[mutate.0] = mutate.1;

        let refused = send(&harness.app, Method::PUT, &profile_uri, Some(view)).await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label} must be refused"
        );
        assert_eq!(refused.content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
        assert_eq!(refused.json["status"], 422);
        assert_eq!(refused.json["code"], "invalid_request");
        assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);
        assert_eq!(
            harness.store.snapshot(PROFILE_ID),
            Some(before),
            "{label} must not mutate storage"
        );
    }
}

#[tokio::test]
async fn replacement_names_an_unknown_nested_mandatory_field() {
    let harness = Harness::new();
    harness.store.seed(seeded_profile());
    let profile_uri = format!("/api/v1/environment-profiles/{PROFILE_ID}");
    let mut view = send(&harness.app, Method::GET, &profile_uri, None)
        .await
        .json;
    view["mandatory"]["servicez"] = json!([]);

    let refused = send(&harness.app, Method::PUT, &profile_uri, Some(view)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
    assert_eq!(refused.json["code"], "invalid_request");
    assert_eq!(refused.json["field"], "servicez");
    assert_eq!(refused.json["path"], "mandatory.servicez");
    assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn create_still_refuses_server_owned_profile_view_fields() {
    for field in ["id", "mandatory", "created_at", "updated_at", "preferred"] {
        let harness = Harness::new();
        let mut body = serde_json::to_value(field_complete_request()).expect("request serializes");
        body[field] = match field {
            "id" => json!(PROFILE_ID),
            "mandatory" => json!({ "packages": [], "skills": [] }),
            "created_at" | "updated_at" => json!("2026-08-25T00:00:00Z"),
            "preferred" => json!(false),
            _ => unreachable!(),
        };

        let refused = send(
            &harness.app,
            Method::POST,
            "/api/v1/environment-profiles",
            Some(body),
        )
        .await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refused.content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
        assert_eq!(refused.json["code"], "invalid_request");
        assert_eq!(refused.json["field"], field);
        assert_eq!(harness.store.len(), 0);
    }
}

#[tokio::test]
async fn retired_mandatory_identities_only_bypass_bounds_for_an_exact_projection() {
    let harness = Harness::with_baseline(Baseline::new(
        [SoftwarePackage::new("new-mandatory-package")],
        [SkillId::new("new-mandatory-skill")],
        [ServiceId::new("new-mandatory-service")],
    ));
    harness.store.seed(retired_baseline_boundary_profile());
    let stored_before = harness
        .store
        .snapshot(PROFILE_ID)
        .expect("the historical profile is stored");
    let profile_uri = format!("/api/v1/environment-profiles/{PROFILE_ID}");

    let before = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(before.json["packages"].as_array().unwrap().len(), 102);
    assert_eq!(
        before.json["agent"]["skills"].as_array().unwrap().len(),
        102
    );
    assert_eq!(
        before.json["nyxid"]["services"].as_array().unwrap().len(),
        102
    );
    assert_eq!(before.json["packages"][0]["name"], "new-mandatory-package");
    assert!(before.json["packages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|package| package["name"] == "old-mandatory-package"));
    assert_eq!(before.json["agent"]["skills"][0], "new-mandatory-skill");
    assert!(before.json["agent"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill == "old-mandatory-skill"));
    assert_eq!(before.json["nyxid"]["services"][0], "new-mandatory-service");
    assert!(before.json["nyxid"]["services"]
        .as_array()
        .unwrap()
        .iter()
        .any(|service| service == "old-mandatory-service"));

    let projection = writable_projection(&before.json);
    let echoed = send(
        &harness.app,
        Method::PUT,
        &profile_uri,
        Some(projection.clone()),
    )
    .await;
    assert_eq!(echoed.status, StatusCode::OK);
    assert_eq!(echoed.bytes, before.bytes);
    assert_eq!(harness.store.upserts.load(Ordering::SeqCst), 0);
    assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);
    assert_eq!(
        harness.store.snapshot(PROFILE_ID),
        Some(stored_before.clone())
    );

    let mut changed_packages = projection.clone();
    changed_packages["name"] = json!("changed packages");
    changed_packages["agent"]["skills"] = json!(["new-mandatory-skill"]);
    changed_packages["nyxid"]["services"] = json!(["new-mandatory-service"]);

    let mut changed_skills = projection.clone();
    changed_skills["name"] = json!("changed skills");
    changed_skills["packages"] = json!([{ "name": "new-mandatory-package" }]);
    changed_skills["nyxid"]["services"] = json!(["new-mandatory-service"]);

    let mut changed_services = projection;
    changed_services["name"] = json!("changed services");
    changed_services["packages"] = json!([{ "name": "new-mandatory-package" }]);
    changed_services["agent"]["skills"] = json!(["new-mandatory-skill"]);

    for (collection, changed) in [
        ("packages", changed_packages),
        ("skills", changed_skills),
        ("services", changed_services),
    ] {
        let refused = send(&harness.app, Method::PUT, &profile_uri, Some(changed)).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refused.content_type.as_deref(), Some(PROBLEM_CONTENT_TYPE));
        assert_eq!(refused.json["status"], 422);
        assert_eq!(refused.json["code"], "invalid_request");
        assert!(
            refused.json["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(collection)),
            "the independent {collection} refusal names its bound: {}",
            refused.json["detail"]
        );
        assert_eq!(harness.store.upserts.load(Ordering::SeqCst), 0);
        assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);
        assert_eq!(
            harness.store.snapshot(PROFILE_ID),
            Some(stored_before.clone()),
            "the refused {collection} replacement does not mutate storage"
        );
    }

    let after = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(after.bytes, before.bytes);
}

#[tokio::test]
async fn maximum_caller_lists_plus_the_baseline_echo_byte_identically_without_a_write() {
    let harness = Harness::with_baseline(Baseline::new(
        [
            SoftwarePackage::new("platform-cli"),
            SoftwarePackage::new("platform-cli"),
        ],
        [
            SkillId::new("mandatory-skill"),
            SkillId::new("mandatory-skill"),
        ],
        [
            ServiceId::new("mandatory-service"),
            ServiceId::new("mandatory-service"),
        ],
    ));
    let request = ProfileRequest {
        name: "boundary profile".into(),
        variables: Vec::new(),
        packages: (0..100)
            .map(|i| PackageBody {
                name: format!("caller-package-{i}"),
                version: None,
            })
            .collect(),
        agent: AgentBody {
            skills: (0..100).map(|i| format!("caller-skill-{i}")).collect(),
            ..AgentBody::default()
        },
        nyxid: NyxIdBody {
            services: (0..100).map(|i| format!("caller-service-{i}")).collect(),
        },
        agent_notes: String::new(),
    };

    let created = send(
        &harness.app,
        Method::POST,
        "/api/v1/environment-profiles",
        Some(serde_json::to_value(request).expect("boundary request serializes")),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.json["packages"].as_array().unwrap().len(), 101);
    assert_eq!(
        created.json["agent"]["skills"].as_array().unwrap().len(),
        101
    );
    assert_eq!(
        created.json["nyxid"]["services"].as_array().unwrap().len(),
        101
    );
    assert_eq!(
        created.json["packages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|package| package["name"] == "platform-cli")
            .count(),
        1
    );
    assert_eq!(
        created.json["agent"]["skills"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|skill| skill.as_str() == Some("mandatory-skill"))
            .count(),
        1
    );
    assert_eq!(
        created.json["nyxid"]["services"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|service| service.as_str() == Some("mandatory-service"))
            .count(),
        1
    );
    assert_eq!(
        created.json["mandatory"],
        json!({
            "packages": ["platform-cli"],
            "skills": ["mandatory-skill"],
        })
    );
    let services = send(
        &harness.app,
        Method::GET,
        "/api/v1/environment-profiles/services",
        None,
    )
    .await;
    assert_eq!(services.status, StatusCode::OK);
    assert_eq!(services.json["mandatory"], json!(["mandatory-service"]));
    let id = created.json["id"].as_str().expect("created id");
    let uri = format!("/api/v1/environment-profiles/{id}");
    let before = send(&harness.app, Method::GET, &uri, None).await;
    assert_eq!(before.status, StatusCode::OK);

    let echoed = send(
        &harness.app,
        Method::PUT,
        &uri,
        Some(writable_projection(&before.json)),
    )
    .await;
    assert_eq!(echoed.status, StatusCode::OK);
    assert_eq!(echoed.bytes, before.bytes);
    assert_eq!(harness.store.upserts.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness.store.replacements.load(Ordering::SeqCst),
        0,
        "the accepted boundary echo remains a no-op"
    );

    let after = send(&harness.app, Method::GET, &uri, None).await;
    assert_eq!(after.bytes, before.bytes);
}

#[tokio::test]
async fn a_legacy_get_put_get_is_byte_identical_after_deployment_defaults_change() {
    let harness = Harness::new();
    harness.store.seed(legacy_profile());
    let stored_before = harness
        .store
        .snapshot(PROFILE_ID)
        .expect("the legacy record is stored");
    let profile_uri = format!("/api/v1/environment-profiles/{PROFILE_ID}");

    let before = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(before.json["agent"]["kernel"], "deployment-kernel");
    assert_eq!(before.json["packages"], json!([{ "name": "platform-cli" }]));
    assert_eq!(before.json["agent"]["skills"], json!(["mandatory-skill"]));
    assert_eq!(
        before.json["nyxid"]["services"],
        json!(["mandatory-service"])
    );
    let listed = send(
        &harness.app,
        Method::GET,
        "/api/v1/environment-profiles",
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json, json!([before.json.clone()]));

    let replaced = send(
        &harness.app,
        Method::PUT,
        &profile_uri,
        Some(writable_projection(&before.json)),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.bytes, before.bytes);
    assert_eq!(harness.store.upserts.load(Ordering::SeqCst), 0);
    assert_eq!(harness.store.replacements.load(Ordering::SeqCst), 0);
    assert_eq!(
        harness.store.snapshot(PROFILE_ID),
        Some(stored_before),
        "normalizing a legacy read and echo does not migrate storage or advance updated_at"
    );

    let after = send(&harness.app, Method::GET, &profile_uri, None).await;
    assert_eq!(after.bytes, before.bytes);
}

#[tokio::test]
async fn replacement_omissions_reset_writable_fields_to_defaults() {
    let harness = Harness::new();
    harness.store.seed(seeded_profile());

    let replaced = send(
        &harness.app,
        Method::PUT,
        &format!("/api/v1/environment-profiles/{PROFILE_ID}"),
        Some(json!({ "name": "replacement" })),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json["name"], "replacement");
    assert_eq!(replaced.json["variables"], json!([]));
    assert_eq!(
        replaced.json["packages"],
        json!([{ "name": "platform-cli" }])
    );
    assert_eq!(replaced.json["agent"]["kernel"], "deployment-kernel");
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
    for (unknown, path, request) in [
        (
            "unexpected_profile_field",
            "unexpected_profile_field",
            json!({ "name": "profile", "unexpected_profile_field": true }),
        ),
        (
            "variableValue",
            "variables[0].variableValue",
            json!({
                "name": "profile",
                "variables": [{ "key": "A", "value": "1", "variableValue": "1" }],
            }),
        ),
        (
            "packageVersion",
            "packages[0].packageVersion",
            json!({
                "name": "profile",
                "packages": [{ "name": "ripgrep", "packageVersion": "14" }],
            }),
        ),
        (
            "reasoningEffort",
            "agent.reasoningEffort",
            json!({ "name": "profile", "agent": { "reasoningEffort": "low" } }),
        ),
        (
            "memoryMb",
            "agent.size.memoryMb",
            json!({
                "name": "profile",
                "agent": {
                    "size": { "cpu_millis": 500, "memory_mib": 512, "memoryMb": 512 },
                },
            }),
        ),
        (
            "serviceIds",
            "nyxid.serviceIds",
            json!({ "name": "profile", "nyxid": { "serviceIds": ["calendar"] } }),
        ),
    ] {
        let diagnostic =
            unknown_profile_field(&request, false).expect("the unknown field is located");
        assert_eq!(diagnostic.field.as_deref(), Some(unknown));
        assert_eq!(diagnostic.path.as_deref(), Some(path));

        let message = serde_json::from_value::<ProfileRequest>(request)
            .expect_err("an unknown field must be refused")
            .to_string();
        assert!(
            message.contains(unknown),
            "schema refusal must identify {unknown}, got {message}"
        );
    }
}

#[test]
fn arbitrary_unknown_key_text_is_not_reflected_as_schema_metadata() {
    let submitted = "secret@example.com=do-not-reflect";
    let mut body = json!({ "name": "profile" });
    body.as_object_mut()
        .expect("the request is an object")
        .insert(submitted.into(), true.into());
    let diagnostic =
        unknown_profile_field(&body, false).expect("the unknown field is still detected");

    assert!(diagnostic.field.is_none());
    assert!(diagnostic.path.is_none());
}

#[tokio::test]
async fn create_and_replace_unknown_fields_are_confidential_problem_documents() {
    for (unknown, path, submitted_value, body) in [
        (
            "unexpected_profile_field",
            "unexpected_profile_field",
            "do-not-reflect-top-level",
            json!({
                "name": "profile",
                "unexpected_profile_field": "do-not-reflect-top-level",
            }),
        ),
        (
            "reasoningEffort",
            "agent.reasoningEffort",
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
            assert_eq!(refused.json["field"], unknown);
            assert_eq!(refused.json["path"], path);
            assert_eq!(
                refused.json["detail"],
                "The JSON body does not match this operation's request schema."
            );
            let rendered = String::from_utf8(refused.bytes).expect("problem JSON is UTF-8");
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

#[test]
fn environment_profile_documentation_examples_follow_the_wire_types() {
    let section = environment_profile_documentation();
    for route in [
        "POST /api/v1/environment-profiles",
        "GET /api/v1/environment-profiles?limit=50",
        "GET /api/v1/environment-profiles/{id}",
        "PUT /api/v1/environment-profiles/{id}",
        "DELETE /api/v1/environment-profiles/{id}",
        "GET /api/v1/environment-profiles/skills",
        "GET /api/v1/environment-profiles/services",
        "PUT /api/v1/environment-profiles/preferred",
    ] {
        assert!(section.contains(route), "the documentation covers {route}");
    }

    let requests = marked_examples(section, "profile-request-example");
    assert!(
        requests.len() >= 2,
        "create and replace are both exemplified"
    );
    for request in &requests {
        serde_json::from_value::<ProfileRequest>(request.clone())
            .expect("documented profile requests match the write shape");
    }
    assert_eq!(
        requests[0],
        serde_json::to_value(field_complete_request()).expect("complete request serializes"),
        "the canonical request example enumerates every accepted field"
    );

    let invalid = marked_examples(section, "profile-invalid-request-example");
    assert_eq!(invalid.len(), 1);
    serde_json::from_value::<ProfileRequest>(invalid[0].clone())
        .expect_err("the documented unknown-field example must really be refused");

    let preference = marked_examples(section, "preferred-request-example");
    assert_eq!(preference.len(), 2, "selection and clearing are both shown");
    let selected: super::PreferredRequest =
        serde_json::from_value(preference[0].clone()).expect("selection request parses");
    assert_eq!(selected.profile.as_deref(), Some("envp_example"));
    let cleared: super::PreferredRequest =
        serde_json::from_value(preference[1].clone()).expect("clear request parses");
    assert!(preference[1].get("profile").is_some_and(Value::is_null));
    assert!(cleared.profile.is_none());

    let harness = Harness::new();
    let profile_view = serde_json::to_value(ProfileView::render(
        documented_profile(false),
        &harness.service,
    ))
    .expect("ProfileView serializes");
    assert_eq!(
        marked_examples(section, "profile-view-example"),
        vec![profile_view.clone()],
        "the response example is produced by ProfileView's real serializer"
    );
    let (decoded, echoed_id) = decode_profile_request(profile_view.clone(), true)
        .expect("a complete ProfileView is accepted by the replacement boundary");
    assert_eq!(echoed_id.as_deref(), profile_view["id"].as_str());
    assert_eq!(
        serde_json::to_value(decoded).expect("decoded request serializes"),
        writable_projection(&profile_view),
        "server-owned fields are stripped without changing writable contents"
    );
    assert_eq!(
        marked_examples(section, "profile-list-response-example"),
        vec![json!([profile_view])],
        "the collection example contains complete ProfileView rows"
    );

    let preferred_view = serde_json::to_value(ProfileView::render(
        documented_profile(true),
        &harness.service,
    ))
    .expect("preferred ProfileView serializes");
    assert_eq!(
        marked_examples(section, "preferred-response-example"),
        vec![json!([preferred_view])],
        "the preferred response is a complete profile collection"
    );
}

#[tokio::test]
async fn documented_collection_catalogue_and_preference_responses_are_served() {
    let section = environment_profile_documentation();
    let create_harness = Harness::new();
    let request = marked_examples(section, "profile-request-example")[0].clone();
    let created = send(
        &create_harness.app,
        Method::POST,
        "/api/v1/environment-profiles",
        Some(request.clone()),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.content_type.as_deref(), Some("application/json"));
    assert!(created.json["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("envp_")));
    assert_eq!(created.json["created_at"], created.json["updated_at"]);
    assert_eq!(created.json["preferred"], false);
    assert_eq!(writable_projection(&created.json), request);
    let mut expected_created = marked_examples(section, "profile-view-example")[0].clone();
    expected_created["id"] = created.json["id"].clone();
    expected_created["created_at"] = created.json["created_at"].clone();
    expected_created["updated_at"] = created.json["updated_at"].clone();
    assert_eq!(
        created.json, expected_created,
        "the documented 201 example has the route's complete real response shape"
    );
    assert_eq!(create_harness.store.upserts.load(Ordering::SeqCst), 1);
    assert_eq!(create_harness.store.replacements.load(Ordering::SeqCst), 0);

    let harness = Harness::new();
    harness.store.seed(documented_profile(false));

    let read = send(
        &harness.app,
        Method::GET,
        "/api/v1/environment-profiles/envp_example",
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(
        read.json,
        marked_examples(section, "profile-view-example")[0]
    );

    let list = send(
        &harness.app,
        Method::GET,
        "/api/v1/environment-profiles?limit=50",
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(
        list.json,
        marked_examples(section, "profile-list-response-example")[0]
    );

    let skills = send(
        &harness.app,
        Method::GET,
        "/api/v1/environment-profiles/skills",
        None,
    )
    .await;
    assert_eq!(skills.status, StatusCode::OK);
    assert_eq!(
        skills.json,
        marked_examples(section, "skill-catalog-response-example")[0]
    );

    let services = send(
        &harness.app,
        Method::GET,
        "/api/v1/environment-profiles/services",
        None,
    )
    .await;
    assert_eq!(services.status, StatusCode::OK);
    assert_eq!(
        services.json,
        marked_examples(section, "service-catalog-response-example")[0]
    );

    let preferred = send(
        &harness.app,
        Method::PUT,
        "/api/v1/environment-profiles/preferred",
        Some(json!({ "profile": "envp_example" })),
    )
    .await;
    assert_eq!(preferred.status, StatusCode::OK);
    assert_eq!(
        preferred.json,
        marked_examples(section, "preferred-response-example")[0]
    );

    let cleared = send(
        &harness.app,
        Method::PUT,
        "/api/v1/environment-profiles/preferred",
        Some(json!({ "profile": null })),
    )
    .await;
    assert_eq!(cleared.status, StatusCode::OK);
    assert!(cleared
        .json
        .as_array()
        .expect("preference response is a collection")
        .iter()
        .all(|profile| profile["preferred"] == false));
}

#[tokio::test]
async fn configured_catalogue_outages_are_route_specific_503_problem_documents() {
    let section = environment_profile_documentation();
    assert!(section.contains("`503 skill_catalogue_unavailable`"));
    assert!(section.contains("`503 service_catalogue_unavailable`"));
    let harness = Harness::with_unavailable_catalogs();

    for (uri, code) in [
        (
            "/api/v1/environment-profiles/skills",
            "skill_catalogue_unavailable",
        ),
        (
            "/api/v1/environment-profiles/services",
            "service_catalogue_unavailable",
        ),
    ] {
        let response = send(&harness.app, Method::GET, uri, None).await;
        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert_eq!(
            response.content_type.as_deref(),
            Some(PROBLEM_CONTENT_TYPE),
            "{uri}"
        );
        assert_eq!(response.json["code"], code, "{uri}");
    }
}

#[tokio::test]
async fn profile_identifier_refusals_and_owned_deletion_have_demonstrated_statuses() {
    let harness = Harness::new();
    let mut foreign = seeded_profile();
    foreign.id = EnvironmentProfileId("envp_foreign".into());
    foreign.owner = SubjectId("subject-somebody-else".into());
    harness.store.seed(foreign);

    for id in ["envp_unknown", "envp_foreign"] {
        let uri = format!("/api/v1/environment-profiles/{id}");
        for (method, body) in [
            (Method::GET, None),
            (
                Method::PUT,
                Some(json!({
                    "name": "replacement",
                    "agent": { "kernel": "profile-kernel" },
                })),
            ),
            (Method::DELETE, None),
        ] {
            let refused = send(&harness.app, method.clone(), &uri, body).await;
            assert_eq!(refused.status, StatusCode::NOT_FOUND, "{method} {uri}");
            assert_eq!(refused.json["code"], "environment_profile_not_found");
        }

        let refused = send(
            &harness.app,
            Method::PUT,
            "/api/v1/environment-profiles/preferred",
            Some(json!({ "profile": id })),
        )
        .await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND);
        assert_eq!(refused.json["code"], "environment_profile_not_found");
    }

    // `mandatory` marks baseline entries within a profile. It does not make
    // the profile itself undeletable, so the issue's proposed refusal is not a
    // status this service actually serves.
    harness.store.seed(seeded_profile());
    let (status, body) = send_without_json(
        &harness.app,
        Method::DELETE,
        &format!("/api/v1/environment-profiles/{PROFILE_ID}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());
    assert!(harness.store.snapshot(PROFILE_ID).is_none());
}
