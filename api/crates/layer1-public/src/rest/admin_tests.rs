use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    response::Response,
    Router,
};
use layer2_core::profiles::store::{AgentProfileStore, InMemoryProfileStore};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, PermissionLevel, ProfileVisibility, SandboxProfileContent,
    SandboxProfileSnapshot, ThinkingEffort,
};
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxCredentialState, SandboxEventKind, SandboxLimits,
    SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
    StoredSandboxRetirement,
};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityService, ActivityStore,
    Authorization, Capability, Page, PlatformSettings, PlatformSettingsService,
    PlatformSettingsStore, ProviderInventory, RuntimeEntry, RuntimeInventory, RuntimeStatus,
    SandboxId, StoreError, StoredPlatformSettings, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::auth::AuthenticatedSubject;

const NOW: &str = "2026-09-05T00:00:00Z";

#[derive(Default)]
struct MemoryActivity {
    records: Mutex<Vec<ActivityRecord>>,
}

#[async_trait]
impl ActivityStore for MemoryActivity {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn list(&self, query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        let mut records: Vec<_> = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                query
                    .actor
                    .as_ref()
                    .is_none_or(|actor| record.actor.as_ref() == Some(actor))
                    && query.action.is_none_or(|action| record.action == action)
            })
            .take(query.limit as usize)
            .cloned()
            .collect();
        if query.cursor.as_deref() == Some("after-first") && !records.is_empty() {
            records.remove(0);
        }
        Ok(Page::new(records, None))
    }
}

struct MemorySettings {
    stored: Mutex<StoredPlatformSettings>,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            stored: Mutex::new(StoredPlatformSettings {
                revision: 0,
                settings: PlatformSettings::default(),
            }),
        }
    }
}

#[async_trait]
impl PlatformSettingsStore for MemorySettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let mut stored = self.stored.lock().unwrap();
        if stored.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        *stored = StoredPlatformSettings {
            revision: expected_revision + 1,
            settings: settings.clone(),
        };
        Ok(stored.clone())
    }
}

struct FixedInventory;

#[async_trait]
impl RuntimeInventory for FixedInventory {
    async fn snapshot(&self) -> Result<ProviderInventory, StoreError> {
        Ok(ProviderInventory {
            observed_at: NOW.into(),
            runtimes: vec![RuntimeEntry {
                runtime_id: "runtime-a".into(),
                generation: 3,
                owner_reference: Some("sbx_a".into()),
                status: RuntimeStatus::Ready,
            }],
        })
    }
}

pub(crate) struct Fixture {
    pub(crate) app: Router,
    pub(crate) sandboxes: Arc<InMemorySandboxStore>,
    pub(crate) profiles: Arc<InMemoryProfileStore>,
    activity: Arc<MemoryActivity>,
    names: Arc<crate::rest::public_names::tests::Names>,
}

pub(crate) fn fixture(inventory: bool) -> Fixture {
    fixture_with_pools(inventory, None)
}

pub(crate) fn fixture_with_pools(
    inventory: bool,
    pools: Option<Arc<dyn warm_pools::WarmPoolAdministration>>,
) -> Fixture {
    let activity = Arc::new(MemoryActivity::default());
    let activity_service = ActivityService::new(activity.clone());
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(MemorySettings::default()),
        Arc::clone(&activity_service),
    ));
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let retirement = Arc::new(StoredSandboxRetirement::new(sandboxes.clone()));
    let operators = Arc::new(layer2_core::sandbox::SandboxOperatorService::new(
        sandboxes.clone(),
        profiles.clone(),
        retirement,
        Arc::clone(&activity_service),
    ));
    let names = crate::rest::public_names::tests::Names::seeded();
    let mut state = AdminState::new(settings, operators, activity_service)
        .with_user_directory(layer2_core::UserDirectory::new(names.clone()))
        .with_runtime_updates(Arc::new(
            layer2_core::sandbox::SandboxRuntimeUpdateService::new(
                sandboxes.clone(),
                "runtime-v2".into(),
            ),
        ));
    if inventory {
        state = state.with_runtime_inventory(Arc::new(FixedInventory));
    }
    if let Some(pools) = pools {
        state = state.with_warm_pools(pools);
    }
    Fixture {
        app: router(state),
        sandboxes,
        profiles,
        activity,
        names,
    }
}

#[tokio::test]
async fn owner_names_follow_authorized_cross_owner_reads_and_never_grant_control() {
    use std::sync::atomic::Ordering;
    let fixture = fixture(false);
    fixture
        .sandboxes
        .create_sandbox(sandbox("sbx_names", "owner-b", "runtime-a"))
        .await
        .unwrap();
    for path in [
        "/api/v1/admin/sandboxes",
        "/api/v1/admin/sandboxes/sbx_names",
        "/api/v1/admin/activity",
    ] {
        let response = send(&fixture.app, Method::GET, path, None, []).await;
        assert!(matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
        assert!(!String::from_utf8(body_bytes(response).await)
            .unwrap()
            .contains("Sam Lee"));
    }
    assert_eq!(fixture.names.calls.load(Ordering::SeqCst), 0);
    let page = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let page = response_json(page).await;
    assert_eq!(page["items"][0]["owner"], "owner-b");
    assert_eq!(page["items"][0]["owner_name"], "Sam Lee");
    let detail = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_names",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    let detail = response_json(detail).await;
    assert_eq!(detail["owner_name"], "Sam Lee");
    assert_eq!(detail["sandbox"]["owner"], "owner-b");
    let activity = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/activity",
        None,
        [Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(
        response_json(activity).await["items"][0]["actor_name"],
        "Morgan Park"
    );
    let refused = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/sandboxes/sbx_names/retire",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
}

fn request(
    method: Method,
    path: &str,
    body: Option<Value>,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    authenticated(builder.body(body).unwrap(), capabilities)
}

fn raw_request(
    method: Method,
    path: &str,
    body: &'static str,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Request<Body> {
    authenticated(
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap(),
        capabilities,
    )
}

fn authenticated(
    mut request: Request<Body>,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Request<Body> {
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new("operator-a")));
    request
        .extensions_mut()
        .insert(Authorization::new(capabilities));
    request
}

async fn body_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

pub(crate) fn sandbox(id: &str, owner: &str, version: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("Sandbox {id}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: layer2_core::SandboxProfileId("sbp_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: layer2_core::profiles::AgentProvider::Codex,
                model_access: layer2_core::profiles::ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: Some(RuntimeBinding {
            boot_id: None,
            provision_generation: Some(1),
            runtime_id: format!("runtime-{id}"),
            runtime_version: version.into(),
            bound_at: NOW.into(),
        }),
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: layer2_core::sandbox::SandboxLifecycleState {
            runtime_generation_high_water: 1,
            ..Default::default()
        },
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(crate) fn agent_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: layer2_core::AgentProfileId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("Agent {id}"),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

async fn send(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
    capabilities: impl IntoIterator<Item = Capability>,
) -> Response {
    app.clone()
        .oneshot(request(method, path, body, capabilities))
        .await
        .unwrap()
}

#[tokio::test]
async fn settings_support_read_replace_presence_aware_patch_and_reset() {
    let fixture = fixture(false);
    let capability = [Capability::ConfigurePlatform];
    let initial = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/settings",
        None,
        capability,
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial = response_json(initial).await;
    assert_eq!(initial["revision"], 0);
    assert!(initial["settings"].get("checkpoints").is_none());
    assert!(initial["settings"]["retention"]
        .get("checkpoint_grace_days")
        .is_none());

    let changed = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": false}
        })),
        capability,
    )
    .await;
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = response_json(changed).await;
    assert_eq!(changed["revision"], 1);
    assert_eq!(changed["settings"]["marketplace"]["enabled"], false);
    assert!(changed["settings"].get("checkpoints").is_none());
    assert!(changed["settings"]["retention"]
        .get("checkpoint_grace_days")
        .is_none());
    assert_eq!(
        changed["settings"]["inactivity"],
        initial["settings"]["inactivity"]
    );

    let mut replacement = changed["settings"].clone();
    replacement["limits"]["max_sandboxes_per_user"] = 9.into();
    let replaced = send(
        &fixture.app,
        Method::PUT,
        "/api/v1/admin/settings",
        Some(json!({"expected_revision": 1, "settings": replacement})),
        capability,
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(
        response_json(replaced).await["settings"]["limits"]["max_sandboxes_per_user"],
        9
    );

    let reset = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/settings/reset",
        Some(json!({"expected_revision": 2})),
        capability,
    )
    .await;
    assert_eq!(reset.status(), StatusCode::OK);
    let reset = response_json(reset).await;
    assert_eq!(reset["revision"], 3);
    assert_eq!(reset["settings"], initial["settings"]);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn settings_refusals_are_confidential_specific_and_revision_safe() {
    let fixture = fixture(false);
    let capability = [Capability::ConfigurePlatform];
    let unknown = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": true, "private_token": "must-not-return"}
        })),
        capability,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let unknown = response_json(unknown).await;
    assert_eq!(unknown["field"], "private_token");
    assert_eq!(unknown["path"], "marketplace.private_token");
    assert!(!unknown.to_string().contains("must-not-return"));

    for body in [
        json!({"expected_revision": 0}),
        json!({"expected_revision": 0, "marketplace": null}),
        json!({"expected_revision": 0, "inactivity": {"pause_after_seconds": 59}}),
        json!({"expected_revision": 0, "checkpoints": {"interval_seconds": 1800}}),
    ] {
        let response = send(
            &fixture.app,
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(body),
            capability,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response_json(response).await["code"], "invalid_request");
    }

    let malformed = fixture
        .app
        .clone()
        .oneshot(raw_request(
            Method::PATCH,
            "/api/v1/admin/settings",
            r#"{"expected_revision":0,"marketplace":"credential-value""#,
            capability,
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let malformed = response_json(malformed).await;
    assert_eq!(malformed["code"], "malformed_json");
    assert!(!malformed.to_string().contains("credential-value"));

    let written = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": false}
        })),
        capability,
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    let stale = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": true}
        })),
        capability,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(stale).await["code"], "settings_conflict");
}

#[tokio::test]
async fn collection_operations_return_the_explicit_operator_refusal() {
    let fixture = fixture(true);
    for (method, path, body) in [
        (Method::GET, "/api/v1/admin/settings", None),
        (
            Method::PUT,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "settings": settings::PlatformSettingsBody::from(PlatformSettings::default())
            })),
        ),
        (
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "marketplace": {"enabled": false}
            })),
        ),
        (
            Method::POST,
            "/api/v1/admin/settings/reset",
            Some(json!({"expected_revision": 0})),
        ),
        (Method::GET, "/api/v1/admin/sandboxes", None),
        (Method::GET, "/api/v1/admin/profiles", None),
        (Method::GET, "/api/v1/admin/runtime-inventory", None),
        (Method::GET, "/api/v1/admin/activity", None),
        (Method::POST, "/api/v1/admin/triggers/pause", None),
        (Method::POST, "/api/v1/admin/triggers/resume", None),
    ] {
        let response = send(&fixture.app, method, path, body, []).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let body = response_json(response).await;
        assert!(body["type"].as_str().unwrap().ends_with("/not-an-operator"));
    }
}

#[tokio::test]
async fn trigger_controls_project_the_platform_kill_switch() {
    let fixture = fixture(false);
    let paused = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/triggers/pause",
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(paused.status(), StatusCode::OK);
    let paused = response_json(paused).await;
    assert_eq!(paused["revision"], 1);
    assert_eq!(paused["settings"]["triggers"]["orchestration_paused"], true);

    let resumed = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/triggers/resume",
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed = response_json(resumed).await;
    assert_eq!(resumed["revision"], 2);
    assert_eq!(
        resumed["settings"]["triggers"]["orchestration_paused"],
        false
    );
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn identifier_refusals_are_byte_identical_to_missing_targets() {
    let fixture = fixture(false);
    fixture
        .sandboxes
        .create_sandbox(sandbox("sbx_exists", "owner-a", "v2"))
        .await
        .unwrap();
    fixture
        .profiles
        .create_agent_profile(agent_profile(
            "agp_exists",
            "owner-a",
            ProfileVisibility::Private,
        ))
        .await
        .unwrap();

    for (method, existing, missing, capability) in [
        (
            Method::GET,
            "/api/v1/admin/sandboxes/sbx_exists",
            "/api/v1/admin/sandboxes/sbx_missing",
            Capability::ReadAllSandboxes,
        ),
        (
            Method::GET,
            "/api/v1/admin/sandboxes/sbx_exists/events",
            "/api/v1/admin/sandboxes/sbx_missing/events",
            Capability::ReadAllSandboxes,
        ),
        (
            Method::POST,
            "/api/v1/admin/sandboxes/sbx_exists/retire",
            "/api/v1/admin/sandboxes/sbx_missing/retire",
            Capability::ControlAnySandbox,
        ),
        (
            Method::POST,
            "/api/v1/admin/profiles/agent/agp_exists/retire",
            "/api/v1/admin/profiles/agent/agp_missing/retire",
            Capability::ControlAnySandbox,
        ),
    ] {
        let hidden = send(&fixture.app, method.clone(), existing, None, []).await;
        let absent = send(&fixture.app, method, missing, None, [capability]).await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND, "{existing}");
        assert_eq!(absent.status(), StatusCode::NOT_FOUND, "{missing}");
        assert_eq!(
            body_bytes(hidden).await,
            body_bytes(absent).await,
            "{existing}"
        );
    }
}

#[tokio::test]
async fn sandbox_routes_filter_read_events_and_retire_idempotently() {
    let fixture = fixture(false);
    let wanted = sandbox("sbx_wanted", "owner-a", "v2");
    fixture
        .sandboxes
        .create_sandbox(wanted.clone())
        .await
        .unwrap();
    fixture
        .sandboxes
        .create_sandbox(sandbox("sbx_other", "owner-b", "v1"))
        .await
        .unwrap();
    fixture
        .sandboxes
        .append_sandbox_event(&wanted.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();

    let listed = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes?owner=owner-a&status=active&runtime_version=v2",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["id"], "sbx_wanted");
    assert_eq!(listed["items"][0]["agent_count"], 0);

    let detail = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_wanted",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = response_json(detail).await;
    assert_eq!(detail["sandbox"]["owner"], "owner-a");
    assert_eq!(detail["workspaces"], json!([]));
    assert_eq!(detail["agents"], json!([]));

    let events = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_wanted/events?after=0&limit=10",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(events.status(), StatusCode::OK);
    assert_eq!(response_json(events).await[0]["kind"]["type"], "created");

    let first = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/sandboxes/sbx_wanted/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = body_bytes(first).await;
    let repeated = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/sandboxes/sbx_wanted/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(first, body_bytes(repeated).await);
    let events = fixture
        .sandboxes
        .sandbox_events(&wanted.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn profile_routes_filter_retire_and_protect_deployment_defaults() {
    let fixture = fixture(false);
    for profile in [
        agent_profile("agp_public", "owner-a", ProfileVisibility::Public),
        agent_profile("agp_private", "owner-b", ProfileVisibility::Private),
        agent_profile(
            layer2_core::DEFAULT_AGENT_PROFILE_ID,
            "system",
            ProfileVisibility::Public,
        ),
    ] {
        fixture
            .profiles
            .create_agent_profile(profile)
            .await
            .unwrap();
    }
    let listed = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/profiles?kind=agent&visibility=public&owner=owner-a",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["id"], "agp_public");

    let first = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/profiles/agent/agp_public/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = body_bytes(first).await;
    let repeated = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/profiles/agent/agp_public/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(first, body_bytes(repeated).await);

    let protected = send(
        &fixture.app,
        Method::POST,
        &format!(
            "/api/v1/admin/profiles/agent/{}/retire",
            layer2_core::DEFAULT_AGENT_PROFILE_ID
        ),
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(protected.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(protected).await["code"], "profile_protected");

    let system = fixture
        .profiles
        .list_agent_profiles(
            Some(&SubjectId::new("system")),
            false,
            &layer2_core::profiles::ProfilePageRequest::default(),
        )
        .await
        .unwrap();
    assert_eq!(system.items[0].retired_at, None);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn runtime_inventory_and_activity_each_require_their_exact_capability() {
    let available = fixture(true);
    available
        .activity
        .append(
            &ActivityRecord::new(ActivityAction::Configure, ActivityOutcome::Succeeded)
                .by(SubjectId::new("operator-a"))
                .on(ActivityTarget::Subject(SubjectId::new("owner-b"))),
        )
        .await
        .unwrap();

    let wrong = send(
        &available.app,
        Method::GET,
        "/api/v1/admin/runtime-inventory",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let inventory = send(
        &available.app,
        Method::GET,
        "/api/v1/admin/runtime-inventory",
        None,
        [Capability::ReadRuntimeInventory],
    )
    .await;
    assert_eq!(inventory.status(), StatusCode::OK);
    let inventory = response_json(inventory).await;
    assert_eq!(inventory["runtimes"][0]["generation"], 3);
    assert_eq!(inventory["runtimes"][0]["status"], "ready");

    let activity = send(
        &available.app,
        Method::GET,
        "/api/v1/admin/activity?action=configure&limit=1",
        None,
        [Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(activity.status(), StatusCode::OK);
    let activity = response_json(activity).await;
    assert_eq!(activity["items"][0]["action"], "configure");
    assert_eq!(activity["items"][0]["actor_name"], "Morgan Park");
    assert_eq!(activity["items"][0]["target_name"], "Sam Lee");

    let unavailable = fixture(false);
    let response = send(
        &unavailable.app,
        Method::GET,
        "/api/v1/admin/runtime-inventory",
        None,
        [Capability::ReadRuntimeInventory],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[path = "admin/warm_pools_tests.rs"]
mod warm_pool_tests;

#[path = "admin/runtime_update_tests.rs"]
mod runtime_update_tests;
