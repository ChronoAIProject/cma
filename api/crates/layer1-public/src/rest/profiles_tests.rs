use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    profiles::{InMemoryProfileStore, ProfileCapacityBounds},
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Capability,
    OperationClaim, OperationReservation, OperationStore, Page, PlatformSettings,
    PlatformSettingsService, PlatformSettingsStore, StoreError, StoredPlatformSettings, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{profiles_router, ProfileRoutePolicy, ProfilesState};
use crate::AuthenticatedSubject;

#[path = "profiles_availability_tests.rs"]
mod availability_tests;
#[path = "defaults_tests.rs"]
mod defaults_tests;
#[path = "profiles_internal_tests.rs"]
mod internal_tests;
#[path = "public_revision_tests.rs"]
mod public_revision_tests;
#[path = "profiles_public_values_tests.rs"]
mod public_values_tests;
#[path = "profiles_reference_tests.rs"]
mod reference_tests;

type OperationKey = (String, String, String);
type OperationRecord = ([u8; 32], Option<Vec<u8>>);

#[derive(Default)]
struct MemoryOperationStore {
    records: Mutex<HashMap<OperationKey, OperationRecord>>,
}

#[async_trait]
impl OperationStore for MemoryOperationStore {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        let key = (
            claim.subject().to_string(),
            claim.operation().to_owned(),
            claim.key().to_owned(),
        );
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match records.get(&key) {
            Some((fingerprint, _)) if fingerprint != claim.fingerprint() => {
                Ok(OperationReservation::FingerprintMismatch)
            }
            Some((_, Some(outcome))) => Ok(OperationReservation::Complete(outcome.clone())),
            Some((_, None)) => Ok(OperationReservation::Pending),
            None => {
                records.insert(key, (*claim.fingerprint(), None));
                Ok(OperationReservation::Acquired)
            }
        }
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        let key = (
            claim.subject().to_string(),
            claim.operation().to_owned(),
            claim.key().to_owned(),
        );
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = records
            .get_mut(&key)
            .ok_or_else(|| StoreError::Malformed("operation claim disappeared".into()))?;
        if record.0 != *claim.fingerprint() {
            return Err(StoreError::Malformed(
                "operation claim fingerprint changed".into(),
            ));
        }
        record.1 = Some(outcome);
        Ok(())
    }
}

struct MemorySettingsStore(Mutex<StoredPlatformSettings>);

impl MemorySettingsStore {
    fn new(marketplace_enabled: bool) -> Self {
        let mut settings = PlatformSettings::default();
        settings.marketplace.enabled = marketplace_enabled;
        Self(Mutex::new(StoredPlatformSettings {
            changes: Default::default(),
            revision: 0,
            settings,
        }))
    }

    fn set_marketplace_enabled(&self, enabled: bool) {
        let mut stored = self.0.lock().unwrap();
        stored.revision += 1;
        stored.settings.marketplace.enabled = enabled;
    }
}

#[async_trait]
impl PlatformSettingsStore for MemorySettingsStore {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let mut stored = self.0.lock().unwrap();
        if stored.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        *stored = StoredPlatformSettings {
            changes: Default::default(),
            revision: expected_revision + 1,
            settings: settings.clone(),
        };
        Ok(stored.clone())
    }
}

#[derive(Default)]
struct MemoryActivityStore;

#[async_trait]
impl ActivityStore for MemoryActivityStore {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list(&self, _query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

fn settings_service(store: Arc<MemorySettingsStore>) -> Arc<PlatformSettingsService> {
    PlatformSettingsService::new(store, ActivityService::new(Arc::new(MemoryActivityStore))).into()
}

fn fixture_with_settings(marketplace_enabled: bool) -> (Router, Arc<MemorySettingsStore>) {
    fixture_with_scanner(marketplace_enabled, Arc::new(["SECRET_TOKEN".into()]))
}

fn fixture_with_scanner(
    marketplace_enabled: bool,
    scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
) -> (Router, Arc<MemorySettingsStore>) {
    fixture_with_ports(
        marketplace_enabled,
        scanner,
        Arc::new(MemoryOperationStore::default()),
    )
}

pub(super) fn fixture_with_public_scanner(
    scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
) -> Router {
    fixture_with_scanner(true, scanner).0
}

pub(super) fn fixture_with_operations(operations: Arc<dyn OperationStore>) -> Router {
    fixture_with_ports(true, Arc::new(["SECRET_TOKEN".into()]), operations).0
}

pub(super) fn fixture_with_profile_store<S>(profiles: Arc<S>) -> Router
where
    S: layer2_core::profiles::AgentProfileStore
        + layer2_core::profiles::WorkspaceProfileStore
        + layer2_core::profiles::SandboxProfileStore
        + layer2_core::profiles::MarketplaceStore
        + 'static,
{
    profiles_router(ProfilesState::new(
        profiles,
        Arc::new(MemoryOperationStore::default()),
        settings_service(Arc::new(MemorySettingsStore::new(true))),
        ProfileRoutePolicy {
            capacity_bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 4_000,
                min_memory_mib: 256,
                max_memory_mib: 8_192,
            },
            public_value_scanner: Arc::new(["SECRET_TOKEN".into()]),
        },
    ))
}

fn fixture_with_ports(
    marketplace_enabled: bool,
    scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
    operations: Arc<dyn OperationStore>,
) -> (Router, Arc<MemorySettingsStore>) {
    let settings = Arc::new(MemorySettingsStore::new(marketplace_enabled));
    let profiles = Arc::new(InMemoryProfileStore::default());
    let router = profiles_router(ProfilesState::new(
        profiles,
        operations,
        settings_service(Arc::clone(&settings)),
        ProfileRoutePolicy {
            capacity_bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 4_000,
                min_memory_mib: 256,
                max_memory_mib: 8_192,
            },
            public_value_scanner: scanner,
        },
    ));
    (router, settings)
}

pub(super) fn fixture(marketplace_enabled: bool) -> Router {
    fixture_with_settings(marketplace_enabled).0
}

fn request(
    method: Method,
    path: &str,
    body: Option<Value>,
    subject: Option<(&str, bool)>,
    key: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    if let Some((subject, can_manage)) = subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new(subject)));
        request.extensions_mut().insert(if can_manage {
            Authorization::new([Capability::ManageOwnProfiles])
        } else {
            Authorization::none()
        });
    }
    request
}

async fn response(router: &Router, request: Request<Body>) -> (StatusCode, Bytes) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1_024 * 1_024)
        .await
        .unwrap();
    (status, body)
}

async fn json_response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, body) = response(router, request).await;
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("response was not JSON ({status}): {error}"));
    (status, value)
}

fn agent_body(name: &str) -> Value {
    json!({
        "name": name,
        "description": format!("{name} description"),
        "content": {
            "model": "gpt-5.2-codex",
            "thinking_effort": "high",
            "permission_level": "guarded",
            "skills": [],
            "system_prompt": "Be exact."
        }
    })
}

fn workspace_body(name: &str, agent_id: &str, revision: u32) -> Value {
    json!({
        "name": format!("{name} profile"),
        "description": "Workspace description",
        "content": {
            "location": { "type": "directory", "name": name },
            "name": name,
            "setup_commands": ["git status --short"],
            "agent_profiles": [{ "id": agent_id, "revision": revision }]
        }
    })
}

fn sandbox_body(
    name: &str,
    workspace_id: Option<&str>,
    install_commands: Vec<&str>,
    environment_value: Option<&str>,
) -> Value {
    let workspace_profiles = workspace_id
        .map(|id| vec![json!({ "id": id, "revision": 1 })])
        .unwrap_or_default();
    let environment_variables = environment_value
        .map(|value| vec![json!({ "name": "APP_VALUE", "value": value })])
        .unwrap_or_default();
    json!({
        "name": name,
        "description": format!("{name} description"),
        "content": {
            "cpu_millis": 1_000,
            "memory_mib": 1_024,
            "nyxid_services": [],
            "agent_provider": "codex",
            "model_access": "chronoai_managed",
            "max_workspaces": 4,
            "max_agents_per_workspace": 4,
            "workspace_profiles": workspace_profiles,
            "software_install_commands": install_commands,
            "environment_variables": environment_variables
        }
    })
}

async fn mutate(
    router: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
    subject: &str,
    key: &str,
) -> (StatusCode, Value) {
    json_response(
        router,
        request(method, path, body, Some((subject, true)), Some(key)),
    )
    .await
}

async fn create_agent(router: &Router, subject: &str, key: &str, name: &str) -> Value {
    let (status, profile) = mutate(
        router,
        Method::POST,
        "/api/v1/agent-profiles",
        Some(agent_body(name)),
        subject,
        key,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    profile
}

async fn publish_agent(
    router: &Router,
    subject: &str,
    id: &str,
    draft_version: u64,
    key: &str,
) -> Value {
    let (status, revision) = mutate(
        router,
        Method::POST,
        &format!("/api/v1/agent-profiles/{id}/revisions"),
        Some(json!({ "draft_version": draft_version })),
        subject,
        key,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    revision
}

async fn set_visibility(
    router: &Router,
    subject: &str,
    kind: &str,
    id: &str,
    visibility: &str,
    key: &str,
) -> (StatusCode, Value) {
    mutate(
        router,
        Method::PUT,
        &format!("/api/v1/{kind}/{id}/visibility"),
        Some(json!({ "visibility": visibility })),
        subject,
        key,
    )
    .await
}

#[tokio::test]
async fn own_routes_enforce_auth_capability_keys_and_exact_replay() {
    let router = fixture(true);
    let body = agent_body("Agent-Auth");

    let (status, problem) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/agent-profiles",
            Some(body.clone()),
            None,
            Some("auth-1"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(problem["code"], "not_authenticated");

    let (status, problem) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/agent-profiles",
            Some(body.clone()),
            Some(("owner", false)),
            Some("auth-2"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["code"], "forbidden");

    let (status, problem) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/agent-profiles",
            Some(body.clone()),
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "missing_idempotency_key");

    let first_request = request(
        Method::POST,
        "/api/v1/agent-profiles",
        Some(body.clone()),
        Some(("owner", true)),
        Some("replay-1"),
    );
    let second_request = request(
        Method::POST,
        "/api/v1/agent-profiles",
        Some(body),
        Some(("owner", true)),
        Some("replay-1"),
    );
    let first = response(&router, first_request).await;
    let replay = response(&router, second_request).await;
    assert_eq!(first, replay);
    assert_eq!(first.0, StatusCode::CREATED);

    let (status, problem) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/agent-profiles",
            Some(agent_body("Different-Agent")),
            Some(("owner", true)),
            Some("replay-1"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "idempotency_key_reused");

    let (status, _) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/agent-profiles",
            None,
            Some(("owner", false)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, page) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/marketplace/agent-profiles",
            None,
            Some(("visitor", false)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"], json!([]));
}

#[tokio::test]
async fn agent_profile_routes_cover_cas_revisions_visibility_retirement_and_paging() {
    let router = fixture(true);
    let profile = create_agent(&router, "owner", "agent-create-1", "Agent-A").await;
    let id = profile["id"].as_str().unwrap();

    let (status, read) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/agent-profiles/{id}"),
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["draft_version"], 1);
    assert!(read.get("latest_revision").is_none());

    let mut update = agent_body("Agent-A-updated");
    update["draft_version"] = json!(1);
    let (status, updated) = mutate(
        &router,
        Method::PUT,
        &format!("/api/v1/agent-profiles/{id}"),
        Some(update.clone()),
        "owner",
        "agent-update-1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["draft_version"], 2);

    let (status, conflict) = mutate(
        &router,
        Method::PUT,
        &format!("/api/v1/agent-profiles/{id}"),
        Some(update),
        "owner",
        "agent-update-stale",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "profile_version_conflict");

    let revision = publish_agent(&router, "owner", id, 2, "agent-publish-1").await;
    assert_eq!(revision["revision"], 1);
    assert_eq!(revision["content"]["system_prompt"], "Be exact.");

    let (status, revisions) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/agent-profiles/{id}/revisions?limit=1"),
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revisions["items"].as_array().unwrap().len(), 1);

    let (status, read_revision) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/agent-profiles/{id}/revisions/1"),
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read_revision["digest"], revision["digest"]);

    let (status, public) = set_visibility(
        &router,
        "owner",
        "agent-profiles",
        id,
        "public",
        "agent-public-1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public["visibility"], "public");

    let missing = format!("agp_{}", "0".repeat(32));
    let wrong_owner = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/agent-profiles/{id}"),
            None,
            Some(("other", true)),
            None,
        ),
    )
    .await;
    let absent = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/agent-profiles/{missing}"),
            None,
            Some(("other", true)),
            None,
        ),
    )
    .await;
    assert_eq!(wrong_owner, absent);
    assert_eq!(wrong_owner.0, StatusCode::NOT_FOUND);

    let second = create_agent(&router, "owner", "agent-create-2", "Agent-B").await;
    assert_ne!(second["id"], id);
    let (_, page) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/agent-profiles?limit=1",
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["next"].as_str().unwrap();
    let (_, next) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/agent-profiles?limit=1&cursor={cursor}"),
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(next["items"].as_array().unwrap().len(), 1);

    let (status, problem) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/agent-profiles?limit=0",
            None,
            Some(("owner", true)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_profile_page");

    let (status, retired) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/agent-profiles/{id}/retire"),
        None,
        "owner",
        "agent-retire-1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(retired["retired_at"].is_string());
}

#[tokio::test]
async fn workspace_routes_validate_references_on_write_and_publish() {
    let router = fixture(true);
    let agent = create_agent(&router, "owner", "workspace-agent-create", "Agent-Ref").await;
    let agent_id = agent["id"].as_str().unwrap();
    publish_agent(&router, "owner", agent_id, 1, "workspace-agent-publish").await;

    let (status, workspace) = mutate(
        &router,
        Method::POST,
        "/api/v1/workspace-profiles",
        Some(workspace_body("workspace-a", agent_id, 1)),
        "owner",
        "workspace-create-1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = workspace["id"].as_str().unwrap();

    for path in [
        format!("/api/v1/workspace-profiles/{id}"),
        "/api/v1/workspace-profiles?limit=10".into(),
    ] {
        let (status, _) = json_response(
            &router,
            request(Method::GET, &path, None, Some(("owner", true)), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "GET {path}");
    }

    let mut update = workspace_body("workspace-a", agent_id, 1);
    update["draft_version"] = json!(1);
    let (status, updated) = mutate(
        &router,
        Method::PUT,
        &format!("/api/v1/workspace-profiles/{id}"),
        Some(update),
        "owner",
        "workspace-update-1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["draft_version"], 2);

    let mut invalid = workspace_body("workspace-a", &format!("agp_{}", "f".repeat(32)), 1);
    invalid["draft_version"] = json!(2);
    let (status, problem) = mutate(
        &router,
        Method::PUT,
        &format!("/api/v1/workspace-profiles/{id}"),
        Some(invalid),
        "owner",
        "workspace-update-invalid",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "profile_reference_invalid");
    assert_eq!(problem["path"], "content.agent_profiles[0]");

    let (status, revision) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/workspace-profiles/{id}/revisions"),
        Some(json!({ "draft_version": 2 })),
        "owner",
        "workspace-publish-1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(revision["revision"], 1);

    for path in [
        format!("/api/v1/workspace-profiles/{id}/revisions"),
        format!("/api/v1/workspace-profiles/{id}/revisions/1"),
    ] {
        let (status, _) = json_response(
            &router,
            request(Method::GET, &path, None, Some(("owner", true)), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "GET {path}");
    }
    assert_eq!(
        set_visibility(
            &router,
            "owner",
            "workspace-profiles",
            id,
            "public",
            "workspace-public-1",
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        mutate(
            &router,
            Method::POST,
            &format!("/api/v1/workspace-profiles/{id}/retire"),
            None,
            "owner",
            "workspace-retire-1",
        )
        .await
        .0,
        StatusCode::OK
    );

    let external = create_agent(
        &router,
        "publisher",
        "external-agent-create",
        "External-Agent",
    )
    .await;
    let external_id = external["id"].as_str().unwrap();
    publish_agent(
        &router,
        "publisher",
        external_id,
        1,
        "external-agent-publish",
    )
    .await;
    set_visibility(
        &router,
        "publisher",
        "agent-profiles",
        external_id,
        "public",
        "external-agent-public",
    )
    .await;
    let (status, consumer_workspace) = mutate(
        &router,
        Method::POST,
        "/api/v1/workspace-profiles",
        Some(workspace_body("consumer-workspace", external_id, 1)),
        "consumer",
        "consumer-workspace-create",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let consumer_id = consumer_workspace["id"].as_str().unwrap();
    set_visibility(
        &router,
        "publisher",
        "agent-profiles",
        external_id,
        "private",
        "external-agent-private",
    )
    .await;
    let (status, problem) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/workspace-profiles/{consumer_id}/revisions"),
        Some(json!({ "draft_version": 1 })),
        "consumer",
        "consumer-workspace-publish",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "profile_reference_invalid");
    assert_eq!(problem["path"], "content.agent_profiles[0]");
}

#[tokio::test]
async fn sandbox_and_marketplace_routes_cover_bounds_trust_fork_and_publication_rules() {
    let router = fixture(true);
    let agent = create_agent(&router, "owner", "sandbox-agent-create", "Agent-Sandbox").await;
    let agent_id = agent["id"].as_str().unwrap();
    publish_agent(&router, "owner", agent_id, 1, "sandbox-agent-publish").await;
    let (status, workspace) = mutate(
        &router,
        Method::POST,
        "/api/v1/workspace-profiles",
        Some(workspace_body("sandbox-workspace", agent_id, 1)),
        "owner",
        "sandbox-workspace-create",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let workspace_id = workspace["id"].as_str().unwrap();
    let (status, workspace_revision) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/workspace-profiles/{workspace_id}/revisions"),
        Some(json!({ "draft_version": 1 })),
        "owner",
        "sandbox-workspace-publish",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let mut invalid_capacity = sandbox_body("Too-Large", None, vec![], None);
    invalid_capacity["content"]["cpu_millis"] = json!(9_000);
    let (status, problem) = mutate(
        &router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(invalid_capacity),
        "owner",
        "sandbox-capacity-invalid",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "profile_invalid");
    assert_eq!(problem["field"], "cpu_millis");
    assert_eq!(problem["reason"], "cpu_out_of_bounds");

    let (status, sandbox) = mutate(
        &router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(sandbox_body("Sandbox-A", Some(workspace_id), vec![], None)),
        "owner",
        "sandbox-create-1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = sandbox["id"].as_str().unwrap();

    for path in [
        format!("/api/v1/sandbox-profiles/{id}"),
        "/api/v1/sandbox-profiles".into(),
    ] {
        assert_eq!(
            json_response(
                &router,
                request(Method::GET, &path, None, Some(("owner", true)), None),
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    let mut update = sandbox_body("Sandbox-A2", Some(workspace_id), vec![], None);
    update["draft_version"] = json!(1);
    let (status, updated) = mutate(
        &router,
        Method::PUT,
        &format!("/api/v1/sandbox-profiles/{id}"),
        Some(update),
        "owner",
        "sandbox-update-1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["draft_version"], 2);

    let mut invalid_reference = sandbox_body(
        "Sandbox-invalid",
        Some(&format!("wkp_{}", "e".repeat(32))),
        vec![],
        None,
    );
    invalid_reference["draft_version"] = json!(2);
    let (status, problem) = mutate(
        &router,
        Method::PUT,
        &format!("/api/v1/sandbox-profiles/{id}"),
        Some(invalid_reference),
        "owner",
        "sandbox-reference-invalid",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "profile_reference_invalid");
    assert_eq!(problem["path"], "content.workspace_profiles[0]");

    let (status, revision) = mutate(
        &router,
        Method::POST,
        &format!("/api/v1/sandbox-profiles/{id}/revisions"),
        Some(json!({ "draft_version": 2 })),
        "owner",
        "sandbox-publish-1",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(revision["revision"], 1);

    let mut workspace_update = workspace_body("sandbox-workspace", agent_id, 1);
    workspace_update["draft_version"] = json!(1);
    workspace_update["content"]["setup_commands"] = json!(["echo revision-two"]);
    assert_eq!(
        mutate(
            &router,
            Method::PUT,
            &format!("/api/v1/workspace-profiles/{workspace_id}"),
            Some(workspace_update),
            "owner",
            "sandbox-workspace-update",
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        mutate(
            &router,
            Method::POST,
            &format!("/api/v1/workspace-profiles/{workspace_id}/revisions"),
            Some(json!({ "draft_version": 2 })),
            "owner",
            "sandbox-workspace-publish-2",
        )
        .await
        .0,
        StatusCode::CREATED
    );

    for path in [
        format!("/api/v1/sandbox-profiles/{id}/revisions"),
        format!("/api/v1/sandbox-profiles/{id}/revisions/1"),
    ] {
        assert_eq!(
            json_response(
                &router,
                request(Method::GET, &path, None, Some(("owner", true)), None),
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    assert_eq!(
        set_visibility(
            &router,
            "owner",
            "sandbox-profiles",
            id,
            "public",
            "sandbox-public-1",
        )
        .await
        .0,
        StatusCode::OK
    );

    let marketplace_path = format!("/api/v1/marketplace/sandbox-profiles/{id}");
    let (_, owner_entry) = json_response(
        &router,
        request(
            Method::GET,
            &marketplace_path,
            None,
            Some(("owner", false)),
            None,
        ),
    )
    .await;
    let (_, visitor_entry) = json_response(
        &router,
        request(
            Method::GET,
            &marketplace_path,
            None,
            Some(("visitor", false)),
            None,
        ),
    )
    .await;
    assert_eq!(owner_entry["trust"]["requires_confirmation"], false);
    assert_eq!(visitor_entry["trust"]["requires_confirmation"], true);
    assert_eq!(
        visitor_entry["trust"]["commands_digest"],
        visitor_entry["revision"]["digest"]
    );
    assert_eq!(visitor_entry["resolved_profiles"][0]["kind"], "workspace");
    assert_eq!(visitor_entry["resolved_profiles"][0]["id"], workspace_id);
    assert_eq!(visitor_entry["resolved_profiles"][0]["revision"], 1);
    assert_eq!(visitor_entry["resolved_revisions"][0]["kind"], "workspace");
    assert_eq!(
        visitor_entry["resolved_revisions"][0]["name"],
        "sandbox-workspace profile"
    );
    assert_eq!(
        visitor_entry["resolved_revisions"][0]["revision"],
        workspace_revision
    );
    assert_eq!(
        visitor_entry["resolved_revisions"][0]["revision"]["content"]["setup_commands"],
        json!(["git status --short"])
    );
    assert!(visitor_entry["resolved_revisions"][0]
        .get("owner")
        .is_none());

    let fork_path = format!("/api/v1/marketplace/sandbox-profiles/{id}/fork");
    let (status, problem) = json_response(
        &router,
        request(
            Method::POST,
            &fork_path,
            Some(json!({})),
            Some(("visitor", false)),
            Some("sandbox-fork-denied"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["code"], "forbidden");

    let first_fork = response(
        &router,
        request(
            Method::POST,
            &fork_path,
            Some(json!({})),
            Some(("consumer", true)),
            Some("sandbox-fork-1"),
        ),
    )
    .await;
    let replay = response(
        &router,
        request(
            Method::POST,
            &fork_path,
            Some(json!({})),
            Some(("consumer", true)),
            Some("sandbox-fork-1"),
        ),
    )
    .await;
    assert_eq!(first_fork, replay);
    assert_eq!(first_fork.0, StatusCode::CREATED);
    let fork: Value = serde_json::from_slice(&first_fork.1).unwrap();
    assert_eq!(fork["visibility"], "private");
    assert_eq!(fork["forked_from"]["id"], id);
    assert_eq!(fork["forked_from"]["revision"], 1);

    let (status, second_sandbox) = mutate(
        &router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(sandbox_body("Sandbox-B", None, vec![], None)),
        "owner",
        "sandbox-create-2",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let second_id = second_sandbox["id"].as_str().unwrap();
    assert_eq!(
        mutate(
            &router,
            Method::POST,
            &format!("/api/v1/sandbox-profiles/{second_id}/revisions"),
            Some(json!({ "draft_version": 1 })),
            "owner",
            "sandbox-publish-2",
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        set_visibility(
            &router,
            "owner",
            "sandbox-profiles",
            second_id,
            "public",
            "sandbox-public-2",
        )
        .await
        .0,
        StatusCode::OK
    );

    let (_, first_page) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/marketplace/sandbox-profiles?limit=1",
            None,
            Some(("visitor", false)),
            None,
        ),
    )
    .await;
    assert_eq!(first_page["items"].as_array().unwrap().len(), 1);
    let cursor = first_page["next"].as_str().unwrap();
    let cursor: String = url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect();
    let (_, second_page) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/marketplace/sandbox-profiles?limit=1&cursor={cursor}"),
            None,
            Some(("visitor", false)),
            None,
        ),
    )
    .await;
    assert_eq!(second_page["items"].as_array().unwrap().len(), 1);

    let (status, secret_sandbox) = mutate(
        &router,
        Method::POST,
        "/api/v1/sandbox-profiles",
        Some(sandbox_body(
            "Secret-Sandbox",
            None,
            vec![],
            Some("contains SECRET_TOKEN marker"),
        )),
        "owner",
        "sandbox-secret-create",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret_id = secret_sandbox["id"].as_str().unwrap();
    assert_eq!(
        mutate(
            &router,
            Method::POST,
            &format!("/api/v1/sandbox-profiles/{secret_id}/revisions"),
            Some(json!({ "draft_version": 1 })),
            "owner",
            "sandbox-secret-publish",
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (status, refused) = set_visibility(
        &router,
        "owner",
        "sandbox-profiles",
        secret_id,
        "public",
        "sandbox-secret-public",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["code"], "profile_public_refused");
    assert_eq!(refused["reason"], "secret_like_environment_value");
    assert_eq!(refused["field"], "environment_variables.APP_VALUE");
    assert!(!refused.to_string().contains("SECRET_TOKEN"));

    assert_eq!(
        mutate(
            &router,
            Method::POST,
            &format!("/api/v1/sandbox-profiles/{id}/retire"),
            None,
            "owner",
            "sandbox-retire-1",
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        json_response(
            &router,
            request(
                Method::GET,
                &marketplace_path,
                None,
                Some(("visitor", false)),
                None,
            ),
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn marketplace_policy_and_path_vocabulary_fail_closed() {
    let (router, settings) = fixture_with_settings(false);
    let (status, problem) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/marketplace/agent-profiles",
            None,
            Some(("visitor", false)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["code"], "marketplace_disabled");

    settings.set_marketplace_enabled(true);
    let (status, body) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/marketplace/agent-profiles",
            None,
            Some(("visitor", false)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));

    settings.set_marketplace_enabled(false);
    assert_eq!(
        json_response(
            &router,
            request(
                Method::GET,
                "/api/v1/marketplace/agent-profiles",
                None,
                Some(("visitor", false)),
                None,
            ),
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    let enabled = fixture(true);
    let unpublished =
        create_agent(&enabled, "owner", "unpublished-create", "Unpublished-Agent").await;
    let (status, problem) = set_visibility(
        &enabled,
        "owner",
        "agent-profiles",
        unpublished["id"].as_str().unwrap(),
        "public",
        "unpublished-public",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "profile_public_refused");
    assert_eq!(problem["reason"], "profile_unpublished");

    for path in [
        "/api/v1/marketplace/agents",
        "/api/v1/marketplace/agent-profiles/not-an-id",
    ] {
        let (status, problem) = json_response(
            &enabled,
            request(Method::GET, path, None, Some(("visitor", false)), None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(problem["code"], "profile_not_found");
    }
}
