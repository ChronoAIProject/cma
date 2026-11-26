use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method, Request, StatusCode},
    Router,
};
use layer2_core::profiles::store::{
    InMemoryProfileStore, SandboxProfileStore, WorkspaceProfileStore,
};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    ProfileCapacityBounds, ProfileVisibility, SandboxProfile, SandboxProfileContent,
    SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation, WorkspaceProfile,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentStore, InMemorySandboxStore, SandboxEventStore, SandboxStore, WorkspaceStore,
    WorktreeStore,
};
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
    HierarchyReadService, Sandbox, SandboxCheckpointRef, SandboxCredentialState, SandboxEventKind,
    SandboxLifecycleCommandError, SandboxLimits, SandboxOrigin, SandboxPolicy, SandboxService,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep, Workspace,
    WorkspaceOrigin, WorkspaceService, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep, Worktree, WorktreeStatus,
};
use layer2_core::{
    AgentId, AgentProfileId, Authorization, Capability, OperationClaim, OperationReservation,
    OperationStore, SandboxId, SandboxProfileId, StoreError, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{hierarchy_router, HierarchyState};
use crate::rest::instances_problem::HierarchyProblem;
use crate::AuthenticatedSubject;

#[path = "task_activity_tests.rs"]
mod task_activity_tests;

const NOW: &str = "2026-09-05T00:00:00Z";

#[path = "sandbox_capacity_tests.rs"]
mod capacity_tests;
#[path = "sandbox_colors_tests.rs"]
mod colors_tests;
#[path = "sandbox_lifecycle_view_tests.rs"]
mod lifecycle_view_tests;
#[path = "workspace_prepare_recovery_tests.rs"]
pub(super) mod prepare_recovery_tests;
#[path = "sandbox_provider_view_tests.rs"]
mod provider_view_tests;
#[path = "sandbox_runtime_update_tests.rs"]
pub(super) mod runtime_update_tests;
#[path = "sandbox_service_access_tests.rs"]
mod service_access_tests;
#[path = "sandbox_visible_inventory_tests.rs"]
mod visible_inventory_tests;
#[path = "sandbox_visitor_boundary_tests.rs"]
mod visitor_boundary_tests;
type OperationKey = (String, String, String);
type OperationRecord = ([u8; 32], Option<Vec<u8>>);

#[derive(Default)]
pub(super) struct MemoryOperationStore {
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
        let mut records = self.records.lock().unwrap();
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
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&key)
            .ok_or_else(|| StoreError::Malformed("operation claim disappeared".into()))?;
        record.1 = Some(outcome);
        Ok(())
    }
}

pub(super) struct Fixture {
    pub(super) router: Router,
    pub(super) state: HierarchyState,
    pub(super) store: Arc<InMemorySandboxStore>,
    pub(super) profiles: Arc<InMemoryProfileStore>,
    operations: Arc<MemoryOperationStore>,
}

pub(super) fn fixture(sandbox_limit: u16) -> Fixture {
    fixture_with_operations(sandbox_limit, None)
}

pub(super) fn fixture_with_operations(
    sandbox_limit: u16,
    injected: Option<Arc<dyn OperationStore>>,
) -> Fixture {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let sandboxes = SandboxService::new(
        store.clone(),
        profiles.clone(),
        SandboxPolicy {
            max_sandboxes_per_owner: sandbox_limit,
            ..SandboxPolicy::default()
        },
        Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let workspaces = WorkspaceService::new(store.clone(), profiles.clone());
    let hierarchy = HierarchyReadService::new(store.clone());
    let operations = Arc::new(MemoryOperationStore::default());
    let operation_store: Arc<dyn OperationStore> = injected.unwrap_or_else(|| operations.clone());
    let state = HierarchyState::new(sandboxes, workspaces, hierarchy, operation_store)
        .with_user_directory(layer2_core::UserDirectory::new(
            super::public_names::tests::Names::seeded(),
        ));
    let router = hierarchy_router(state.clone());
    Fixture {
        router,
        state,
        store,
        profiles,
        operations,
    }
}

fn all_capabilities() -> Authorization {
    Authorization::new([
        Capability::CreateSandbox,
        Capability::UseOwnSandboxes,
        Capability::DeleteOwnSandbox,
    ])
}

fn request(
    method: Method,
    path: &str,
    body: Option<Value>,
    subject: Option<(&str, Authorization)>,
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
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    if let Some((subject, authorization)) = subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new(subject)));
        request.extensions_mut().insert(authorization);
    }
    request
}

async fn response(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 2 * 1_024 * 1_024)
        .await
        .unwrap();
    (status, headers, body)
}

async fn json_response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, _, body) = response(router, request).await;
    (status, serde_json::from_slice(&body).unwrap())
}

fn authenticated(
    method: Method,
    path: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> Request<Body> {
    request(method, path, body, Some(("owner", all_capabilities())), key)
}

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 250,
        max_cpu_millis: 4_000,
        min_memory_mib: 256,
        max_memory_mib: 8_192,
    }
}

pub(super) async fn publish_sandbox_profile(
    profiles: &InMemoryProfileStore,
    id: &str,
    owner: &str,
    visibility: ProfileVisibility,
    commands: Vec<String>,
) -> (SandboxProfileId, String) {
    let id = SandboxProfileId(id.into());
    profiles
        .create_sandbox_profile(SandboxProfile {
            internal: false,
            availability: Default::default(),
            id: id.clone(),
            owner: SubjectId::new(owner),
            name: format!("Sandbox {id}"),
            description: String::new(),
            visibility,
            forked_from: None,
            uses_count: 0,
            draft: sandbox_content(4),
            draft_version: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let mut profile = profiles.sandbox_profile(&id).await.unwrap().unwrap();
    profile.draft.software_install_commands = commands;
    profile.draft_version = 2;
    profiles.save_sandbox_draft(1, profile).await.unwrap();
    let revision = profiles
        .publish_sandbox_profile(&id, 2, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    (id, revision.digest)
}

pub(super) async fn publish_workspace_profile(
    profiles: &InMemoryProfileStore,
    id: &str,
    owner: &str,
    visibility: ProfileVisibility,
    name: &str,
    commands: Vec<String>,
) -> (WorkspaceProfileId, String) {
    let id = WorkspaceProfileId(id.into());
    profiles
        .create_workspace_profile(WorkspaceProfile {
            internal: false,
            id: id.clone(),
            owner: SubjectId::new(owner),
            name: format!("Workspace {id}"),
            description: String::new(),
            visibility,
            forked_from: None,
            uses_count: 0,
            draft: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: name.into() },
                name: name.into(),
                setup_commands: commands,
                agent_profiles: vec![],
            },
            draft_version: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let revision = profiles
        .publish_workspace_profile(&id, 1, NOW.into())
        .await
        .unwrap();
    (id, revision.digest)
}

fn sandbox_content(max_workspaces: u8) -> SandboxProfileContent {
    SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1_000,
        memory_mib: 2_048,
        nyxid_services: vec![],
        agent_provider: AgentProvider::Codex,
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces,
        max_agents_per_workspace: 4,
        workspace_profiles: vec![],
        software_install_commands: vec![],
        environment_variables: vec![],
    }
}

fn stored_sandbox(id: &str, owner: &str, status: SandboxStatus, max_workspaces: u8) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("sandbox-{id}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_stored".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: sandbox_content(max_workspaces),
        },
        status,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn stored_workspace(parent: &Sandbox, id: &str) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_stored".into()),
            revision: 2,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: format!("project-{id}"),
        path: format!("/home/cma/project-{id}"),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 2,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn stored_worktree(parent: &Workspace, id: &str, agent_id: &str) -> Worktree {
    Worktree {
        id: WorktreeId(id.into()),
        workspace_id: parent.id.clone(),
        sandbox_id: parent.sandbox_id.clone(),
        owner: parent.owner.clone(),
        name: "wt-1".into(),
        path: format!("{}/.worktrees/wt-1", parent.path),
        branch: "cma/wt-1".into(),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(AgentId(agent_id.into())),
        revision: 1,
    }
}

fn stored_agent(parent: &Workspace, tree: &Worktree, id: &str) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: tree.id.clone(),
        workspace_id: parent.id.clone(),
        sandbox_id: parent.sandbox_id.clone(),
        owner: parent.owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_stored".into()),
            revision: 3,
            digest: "e".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::OpenConversation, NOW.into()),
        failure: None,
        title: Some("Hierarchy route".into()),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

async fn insert_hierarchy(
    store: &InMemorySandboxStore,
    sandbox: &Sandbox,
    workspace: Option<&Workspace>,
    tree: Option<&Worktree>,
    agent: Option<&Agent>,
) {
    store.create_sandbox(sandbox.clone()).await.unwrap();
    if let Some(workspace) = workspace {
        store.create_workspace(workspace.clone()).await.unwrap();
    }
    if let Some(tree) = tree {
        store.create_worktree(tree.clone()).await.unwrap();
    }
    if let Some(agent) = agent {
        store.create_agent(agent.clone()).await.unwrap();
    }
}

pub(super) fn checkpoint(epoch: u64) -> SandboxCheckpointRef {
    let object_reference = format!("checkpoints/sandbox-{epoch}.tar.zst");
    let sha256 = format!("{epoch:064x}");
    let sealed_at = "2026-09-05T00:29:00Z".to_owned();
    let mut manifest = Sha256::new();
    manifest.update(b"cma-sandbox-checkpoint-v1\0");
    manifest.update(epoch.to_be_bytes());
    for value in [&object_reference, &sha256] {
        manifest.update(u64::try_from(value.len()).unwrap().to_be_bytes());
        manifest.update(value.as_bytes());
    }
    manifest.update(0_u64.to_be_bytes());
    manifest.update(u64::try_from(sealed_at.len()).unwrap().to_be_bytes());
    manifest.update(sealed_at.as_bytes());
    SandboxCheckpointRef {
        epoch,
        object_reference,
        sha256,
        conversations: vec![],
        sealed_at,
        manifest_sha256: format!("{:x}", manifest.finalize()),
    }
}

pub(super) fn paused_sandbox(id: &str, owner: &str) -> Sandbox {
    let mut sandbox = stored_sandbox(id, owner, SandboxStatus::Paused, 4);
    sandbox.lifecycle.checkpoints.latest = Some(checkpoint(1));
    sandbox
}

#[tokio::test]
async fn sandbox_lifecycle_routes_enforce_auth_and_operation_keys() {
    let fixture = fixture(10);
    let active = stored_sandbox("sbx_lifecycle_auth", "owner", SandboxStatus::Active, 4);
    fixture.store.create_sandbox(active).await.unwrap();
    let path = "/api/v1/sandboxes/sbx_lifecycle_auth/pause";

    let (status, problem) = json_response(
        &fixture.router,
        request(Method::POST, path, None, None, Some("unauthenticated")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(problem["code"], "not_authenticated");

    let (status, problem) = json_response(
        &fixture.router,
        request(
            Method::POST,
            path,
            None,
            Some(("owner", Authorization::none())),
            Some("forbidden"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["code"], "forbidden");

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(Method::POST, path, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "missing_idempotency_key");

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(Method::POST, path, None, Some("contains space")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_idempotency_key");

    let mut duplicate = authenticated(Method::POST, path, None, Some("first"));
    duplicate
        .headers_mut()
        .append("idempotency-key", "second".parse().unwrap());
    let (status, problem) = json_response(&fixture.router, duplicate).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_idempotency_key");

    fixture.operations.records.lock().unwrap().insert(
        (
            "owner".into(),
            "sandbox:pause:sbx_lifecycle_auth".into(),
            "reused".into(),
        ),
        ([0; 32], None),
    );
    let (status, problem) = json_response(
        &fixture.router,
        authenticated(Method::POST, path, None, Some("reused")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "idempotency_key_reused");
}

#[tokio::test]
async fn sandbox_lifecycle_commands_are_bodyless_accepted_and_exactly_replayed() {
    let fixture = fixture(10);
    let active = stored_sandbox("sbx_pause", "owner", SandboxStatus::Active, 4);
    let paused = paused_sandbox("sbx_revive", "owner");
    fixture.store.create_sandbox(active.clone()).await.unwrap();
    fixture.store.create_sandbox(paused.clone()).await.unwrap();

    let mut malformed_json = authenticated(
        Method::POST,
        "/api/v1/sandboxes/sbx_pause/pause",
        None,
        Some("same-key"),
    );
    malformed_json
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    *malformed_json.body_mut() = Body::from("{not valid JSON");
    let paused_response = response(&fixture.router, malformed_json).await;
    let pause_replay = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_pause/pause",
            None,
            Some("same-key"),
        ),
    )
    .await;
    assert_eq!(paused_response, pause_replay);
    assert_eq!(paused_response.0, StatusCode::ACCEPTED);
    let paused_view: Value = serde_json::from_slice(&paused_response.2).unwrap();
    assert_eq!(paused_view["id"], "sbx_pause");
    assert_eq!(paused_view["status"], "pausing");
    assert!(paused_view.get("credential").is_none());
    let stored = fixture
        .store
        .get_sandbox(&active.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, SandboxStatus::Pausing);
    assert_eq!(stored.revision, active.revision + 1);
    assert_eq!(
        fixture
            .store
            .sandbox_events(&active.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );

    let revived_response = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_revive/revive",
            None,
            Some("same-key"),
        ),
    )
    .await;
    let revive_replay = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_revive/revive",
            None,
            Some("same-key"),
        ),
    )
    .await;
    assert_eq!(revived_response, revive_replay);
    assert_eq!(revived_response.0, StatusCode::ACCEPTED);
    let revived_view: Value = serde_json::from_slice(&revived_response.2).unwrap();
    assert_eq!(revived_view["id"], "sbx_revive");
    assert_eq!(revived_view["status"], "reviving");
    let stored = fixture
        .store
        .get_sandbox(&paused.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, SandboxStatus::Reviving);
    assert_eq!(stored.revision, paused.revision + 1);
    assert_eq!(
        fixture
            .store
            .sandbox_events(&paused.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn sandbox_lifecycle_routes_project_conflicts_and_conceal_foreign_ids() {
    let fixture = fixture(10);
    let mut retiring = stored_sandbox("sbx_retiring", "owner", SandboxStatus::Retiring, 4);
    retiring
        .work
        .schedule(SandboxWorkStep::StopAgents, NOW.into());
    let foreign = paused_sandbox("sbx_foreign_lifecycle", "foreign");
    fixture.store.create_sandbox(retiring).await.unwrap();
    fixture.store.create_sandbox(foreign).await.unwrap();

    for (operation, code) in [
        ("pause", "sandbox_not_active"),
        ("revive", "sandbox_not_paused"),
    ] {
        let path = format!("/api/v1/sandboxes/sbx_retiring/{operation}");
        let (status, problem) = json_response(
            &fixture.router,
            authenticated(Method::POST, &path, None, Some(operation)),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["code"], code);

        let foreign_path = format!("/api/v1/sandboxes/sbx_foreign_lifecycle/{operation}");
        let missing_path = format!("/api/v1/sandboxes/sbx_missing_lifecycle/{operation}");
        let foreign_response = response(
            &fixture.router,
            authenticated(
                Method::POST,
                &foreign_path,
                None,
                Some(&format!("foreign-{operation}")),
            ),
        )
        .await;
        let missing_response = response(
            &fixture.router,
            authenticated(
                Method::POST,
                &missing_path,
                None,
                Some(&format!("missing-{operation}")),
            ),
        )
        .await;
        assert_eq!(foreign_response, missing_response);
        assert_eq!(foreign_response.0, StatusCode::NOT_FOUND);
        let problem: Value = serde_json::from_slice(&foreign_response.2).unwrap();
        assert_eq!(problem["code"], "sandbox_not_found");
    }
}

#[tokio::test]
async fn sandbox_lifecycle_problem_projection_preserves_typed_codes() {
    let cases = [
        (
            SandboxLifecycleCommandError::Corrupt("private corrupt detail".into()),
            "stored_sandbox_corrupt",
        ),
        (
            SandboxLifecycleCommandError::Unavailable("private outage detail".into()),
            "sandbox_lifecycle_unavailable",
        ),
    ];
    for (error, code) in cases {
        let response =
            axum::response::IntoResponse::into_response(HierarchyProblem::sandbox_lifecycle(error));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let problem: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], code);
        assert!(!problem.to_string().contains("private"));
    }
}

#[tokio::test]
async fn sandbox_create_enforces_auth_key_trust_limit_and_exact_replay() {
    let fixture = fixture(1);
    let (profile_id, digest) = publish_sandbox_profile(
        &fixture.profiles,
        "sbp_public_unsafe",
        "publisher",
        ProfileVisibility::Public,
        vec!["apt-get update".into()],
    )
    .await;
    let base = json!({
        "sandbox_profile": { "id": profile_id, "revision": 1 },
        "name": "proof-box"
    });

    let (status, problem) = json_response(
        &fixture.router,
        request(
            Method::POST,
            "/api/v1/sandboxes",
            Some(base.clone()),
            None,
            Some("unauth"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(problem["code"], "not_authenticated");

    let (status, problem) = json_response(
        &fixture.router,
        request(
            Method::POST,
            "/api/v1/sandboxes",
            Some(base.clone()),
            Some(("owner", Authorization::new([Capability::UseOwnSandboxes]))),
            Some("forbidden"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["code"], "forbidden");

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(Method::POST, "/api/v1/sandboxes", Some(base.clone()), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "missing_idempotency_key");

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes",
            Some(base.clone()),
            Some("trust-required"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "trust_confirmation_required");
    assert_eq!(problem["commands_digest"], digest);

    let invalid = json!({
        "sandbox_profile": { "id": profile_id, "revision": 1 },
        "commands_digest": digest,
    });
    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes",
            Some(invalid),
            Some("bad-trust"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "invalid_trust_confirmation");

    let confirmed = json!({
        "sandbox_profile": { "id": profile_id, "revision": 1 },
        "name": "proof-box",
        "trust_confirmed": true,
        "commands_digest": digest,
    });
    let first = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes",
            Some(confirmed.clone()),
            Some("create-one"),
        ),
    )
    .await;
    let replay = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes",
            Some(confirmed.clone()),
            Some("create-one"),
        ),
    )
    .await;
    assert_eq!(first, replay);
    assert_eq!(first.0, StatusCode::CREATED);
    let created: Value = serde_json::from_slice(&first.2).unwrap();
    assert!(created.get("credential").is_none());
    assert!(created["profile"].get("content").is_none());

    let mut limited = confirmed;
    limited["name"] = "second-box".into();
    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes",
            Some(limited),
            Some("limit"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(problem["code"], "sandbox_limit_reached");
}

#[tokio::test]
async fn task_navigation_workspace_list_reports_completion_without_reading_conversations() {
    use layer2_core::sandbox::{store::AgentLogStore, AgentLogKind};
    let fixture = fixture(10);
    let sandbox = stored_sandbox("sbx_tasks", "owner", SandboxStatus::Active, 4);
    let workspace = stored_workspace(&sandbox, "wks_tasks");
    let tree = stored_worktree(&workspace, "wkt_tasks", "agt_tasks");
    let mut agent = stored_agent(&workspace, &tree, "agt_tasks");
    agent.status = AgentStatus::Ready;
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    agent.conversation.conversation_id = Some("tasks-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 3;
    insert_hierarchy(
        &fixture.store,
        &sandbox,
        Some(&workspace),
        Some(&tree),
        Some(&agent),
    )
    .await;
    for kind in [
        AgentLogKind::Created {
            workspace_id: workspace.id.clone(),
            worktree_id: tree.id.clone(),
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 2,
            model: None,
            origin_key: None,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 2 },
    ] {
        fixture
            .store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let (status, body) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/workspaces/wks_tasks", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agents"][0]["task_status"], "completed");
    assert!(body["agents"][0].get("task_started_at").is_none());
    assert_eq!(body["agents"][0]["task_activity_at"], NOW);
}

#[tokio::test]
async fn task_clock_uses_its_recorded_turn_start_instead_of_last_metadata_update() {
    use layer2_core::sandbox::{store::AgentLogStore, AgentLogKind};
    let fixture = fixture(10);
    let sandbox = stored_sandbox("sbx_clock", "owner", SandboxStatus::Active, 4);
    let workspace = stored_workspace(&sandbox, "wks_clock");
    let tree = stored_worktree(&workspace, "wkt_clock", "agt_clock");
    let mut agent = stored_agent(&workspace, &tree, "agt_clock");
    agent.status = AgentStatus::Working;
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    agent.conversation.conversation_id = Some("clock-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 2;
    agent.conversation.active_turn = Some(2);
    agent.updated_at = "2026-09-05T00:05:00Z".into();
    insert_hierarchy(
        &fixture.store,
        &sandbox,
        Some(&workspace),
        Some(&tree),
        Some(&agent),
    )
    .await;
    fixture
        .store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: workspace.id.clone(),
                worktree_id: tree.id.clone(),
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .append_agent_log(
            &agent.id,
            "2026-09-05T00:01:00Z".into(),
            AgentLogKind::TurnStarted {
                turn_sequence: 2,
                model: None,
                origin_key: None,
            },
        )
        .await
        .unwrap();
    let (status, body) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/workspaces/wks_clock", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agents"][0]["task_started_at"], "2026-09-05T00:01:00Z");
    assert_eq!(body["agents"][0]["updated_at"], "2026-09-05T00:05:00Z");
}

#[tokio::test]
async fn hierarchy_reads_filter_page_count_and_replay_events() {
    let fixture = fixture(10);
    let first = stored_sandbox("sbx_1", "owner", SandboxStatus::Active, 4);
    let paused = stored_sandbox("sbx_2", "owner", SandboxStatus::Paused, 4);
    let last = stored_sandbox("sbx_3", "owner", SandboxStatus::Active, 4);
    let foreign = stored_sandbox("sbx_0", "foreign", SandboxStatus::Active, 4);
    for sandbox in [&first, &paused, &last, &foreign] {
        fixture.store.create_sandbox(sandbox.clone()).await.unwrap();
    }
    let workspace = stored_workspace(&last, "wks_3");
    let tree = stored_worktree(&workspace, "wkt_3", "agt_3");
    let agent = stored_agent(&workspace, &tree, "agt_3");
    fixture
        .store
        .create_workspace(workspace.clone())
        .await
        .unwrap();
    fixture.store.create_worktree(tree.clone()).await.unwrap();
    fixture.store.create_agent(agent).await.unwrap();
    fixture
        .store
        .append_sandbox_event(&last.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();

    let (status, page) = json_response(
        &fixture.router,
        authenticated(
            Method::GET,
            "/api/v1/sandboxes?status=active&limit=1",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["id"], "sbx_1");
    assert_eq!(page["items"][0]["owner"], "owner");
    assert_eq!(page["items"][0]["owner_name"], "Research Creator");
    assert_eq!(page["next"], "sbx_1");
    let (status, page) = json_response(
        &fixture.router,
        authenticated(
            Method::GET,
            "/api/v1/sandboxes?status=active&limit=1&cursor=sbx_1",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["id"], "sbx_3");
    assert!(page.get("next").is_none());

    let (_, detail) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/sandboxes/sbx_3", None, None),
    )
    .await;
    assert_eq!(detail["workspace_count"], 1);
    assert_eq!(detail["owner"], "owner");
    assert_eq!(detail["owner_name"], "Research Creator");
    assert_eq!(detail["worktree_count"], 1);
    assert_eq!(detail["agent_count"], 1);
    assert_eq!(detail["workspaces"][0]["id"], "wks_3");

    let (_, workspace_detail) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/workspaces/wks_3", None, None),
    )
    .await;
    assert_eq!(workspace_detail["worktrees"][0]["agent_id"], "agt_3");
    assert_eq!(workspace_detail["agents"][0]["id"], "agt_3");
    let (_, worktrees) = json_response(
        &fixture.router,
        authenticated(
            Method::GET,
            "/api/v1/workspaces/wks_3/worktrees",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(worktrees["items"][0]["id"], "wkt_3");
    let (_, worktree) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/worktrees/wkt_3", None, None),
    )
    .await;
    assert_eq!(worktree["agent_id"], "agt_3");

    let (status, events) = json_response(
        &fixture.router,
        authenticated(
            Method::GET,
            "/api/v1/sandboxes/sbx_3/events?after=0&limit=1",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events[0]["seq"], 1);
    let (_, events) = json_response(
        &fixture.router,
        authenticated(
            Method::GET,
            "/api/v1/sandboxes/sbx_3/events?after=1",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(events, json!([]));
}

#[tokio::test]
async fn foreign_and_missing_hierarchy_responses_are_byte_identical() {
    let fixture = fixture(10);
    let sandbox = stored_sandbox("sbx_foreign", "foreign", SandboxStatus::Active, 4);
    let workspace = stored_workspace(&sandbox, "wks_foreign");
    let tree = stored_worktree(&workspace, "wkt_foreign", "agt_foreign");
    let agent = stored_agent(&workspace, &tree, "agt_foreign");
    insert_hierarchy(
        &fixture.store,
        &sandbox,
        Some(&workspace),
        Some(&tree),
        Some(&agent),
    )
    .await;

    for (foreign, missing) in [
        (
            "/api/v1/sandboxes/sbx_foreign",
            "/api/v1/sandboxes/sbx_missing",
        ),
        (
            "/api/v1/sandboxes/sbx_foreign/workspaces",
            "/api/v1/sandboxes/sbx_missing/workspaces",
        ),
        (
            "/api/v1/sandboxes/sbx_foreign/events",
            "/api/v1/sandboxes/sbx_missing/events",
        ),
        (
            "/api/v1/workspaces/wks_foreign",
            "/api/v1/workspaces/wks_missing",
        ),
        (
            "/api/v1/workspaces/wks_foreign/worktrees",
            "/api/v1/workspaces/wks_missing/worktrees",
        ),
        (
            "/api/v1/worktrees/wkt_foreign",
            "/api/v1/worktrees/wkt_missing",
        ),
    ] {
        let foreign = response(
            &fixture.router,
            authenticated(Method::GET, foreign, None, None),
        )
        .await;
        let missing = response(
            &fixture.router,
            authenticated(Method::GET, missing, None, None),
        )
        .await;
        assert_eq!(foreign.0, StatusCode::NOT_FOUND);
        assert_eq!(foreign.2, missing.2);
        assert_eq!(
            foreign.1.get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
    }

    let foreign = response(
        &fixture.router,
        authenticated(
            Method::DELETE,
            "/api/v1/sandboxes/sbx_foreign",
            None,
            Some("delete-foreign"),
        ),
    )
    .await;
    let missing = response(
        &fixture.router,
        authenticated(
            Method::DELETE,
            "/api/v1/sandboxes/sbx_missing",
            None,
            Some("delete-missing"),
        ),
    )
    .await;
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);
    assert_eq!(foreign.2, missing.2);

    let foreign = response(
        &fixture.router,
        authenticated(
            Method::DELETE,
            "/api/v1/workspaces/wks_foreign",
            None,
            Some("remove-foreign"),
        ),
    )
    .await;
    let missing = response(
        &fixture.router,
        authenticated(
            Method::DELETE,
            "/api/v1/workspaces/wks_missing",
            None,
            Some("remove-missing"),
        ),
    )
    .await;
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);
    assert_eq!(foreign.2, missing.2);
}

#[tokio::test]
async fn workspace_mutations_project_conflicts_and_admit_live_child_removal() {
    let fixture = fixture(10);
    let active = stored_sandbox("sbx_active", "owner", SandboxStatus::Active, 1);
    let unready = stored_sandbox("sbx_unready", "owner", SandboxStatus::Provisioning, 1);
    fixture.store.create_sandbox(active.clone()).await.unwrap();
    fixture.store.create_sandbox(unready).await.unwrap();
    let (alpha, _) = publish_workspace_profile(
        &fixture.profiles,
        "wkp_alpha",
        "owner",
        ProfileVisibility::Private,
        "alpha",
        vec![],
    )
    .await;
    let (beta, _) = publish_workspace_profile(
        &fixture.profiles,
        "wkp_beta",
        "owner",
        ProfileVisibility::Private,
        "beta",
        vec![],
    )
    .await;
    let (unsafe_profile, digest) = publish_workspace_profile(
        &fixture.profiles,
        "wkp_unsafe",
        "publisher",
        ProfileVisibility::Public,
        "unsafe",
        vec!["make install".into()],
    )
    .await;

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_unready/workspaces",
            Some(json!({ "workspace_profile": { "id": alpha, "revision": 1 } })),
            Some("unready"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "sandbox_not_ready");

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_active/workspaces",
            Some(json!({ "workspace_profile": { "id": unsafe_profile, "revision": 1 } })),
            Some("workspace-trust"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "trust_confirmation_required");
    assert_eq!(problem["commands_digest"], digest);

    let alpha_body = json!({ "workspace_profile": { "id": alpha, "revision": 1 } });
    let created = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_active/workspaces",
            Some(alpha_body.clone()),
            Some("create-alpha"),
        ),
    )
    .await;
    let replay = response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_active/workspaces",
            Some(alpha_body.clone()),
            Some("create-alpha"),
        ),
    )
    .await;
    assert_eq!(created, replay);
    assert_eq!(created.0, StatusCode::CREATED);
    let created_body: Value = serde_json::from_slice(&created.2).unwrap();
    let created_id = created_body["id"].as_str().unwrap();

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_active/workspaces",
            Some(alpha_body),
            Some("name-conflict"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "workspace_name_conflict");

    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_active/workspaces",
            Some(json!({ "workspace_profile": { "id": beta, "revision": 1 } })),
            Some("workspace-limit"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(problem["code"], "workspace_limit_reached");

    let removable_path = format!("/api/v1/workspaces/{created_id}");
    let removed = response(
        &fixture.router,
        authenticated(Method::DELETE, &removable_path, None, Some("remove-alpha")),
    )
    .await;
    let replay = response(
        &fixture.router,
        authenticated(Method::DELETE, &removable_path, None, Some("remove-alpha")),
    )
    .await;
    assert_eq!(removed, replay);
    assert_eq!(removed.0, StatusCode::ACCEPTED);

    let live_parent = stored_sandbox("sbx_live", "owner", SandboxStatus::Active, 2);
    let live_workspace = stored_workspace(&live_parent, "wks_live");
    let tree = stored_worktree(&live_workspace, "wkt_live", "agt_live");
    let agent = stored_agent(&live_workspace, &tree, "agt_live");
    insert_hierarchy(
        &fixture.store,
        &live_parent,
        Some(&live_workspace),
        Some(&tree),
        Some(&agent),
    )
    .await;
    let (status, removing) = json_response(
        &fixture.router,
        authenticated(
            Method::DELETE,
            "/api/v1/workspaces/wks_live",
            None,
            Some("live-agents"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(removing["id"], "wks_live");
    assert_eq!(removing["status"], "removing");
    let replay = json_response(
        &fixture.router,
        authenticated(
            Method::DELETE,
            "/api/v1/workspaces/wks_live",
            None,
            Some("live-agents"),
        ),
    )
    .await;
    assert_eq!(replay, (status, removing));
}

#[tokio::test]
async fn idempotency_scopes_isolate_workspace_creation_from_sandbox_commands() {
    for command in ["delete", "pause", "revive"] {
        let fixture = fixture(8);
        let create_path = format!("/api/v1/sandboxes/{command}:collision/workspaces");
        let sandbox_path = "/api/v1/sandboxes/collision:workspace:create";
        let (method, command_path) = if command == "delete" {
            (Method::DELETE, sandbox_path.to_owned())
        } else {
            (Method::POST, format!("{sandbox_path}/{command}"))
        };
        let body = json!({"workspace_profile": {"id": "wsp_missing", "revision": 1}});
        let key = Some("same-key-across-operations");
        let create = json_response(
            &fixture.router,
            authenticated(Method::POST, &create_path, Some(body.clone()), key),
        )
        .await;
        assert_eq!(create.0, StatusCode::NOT_FOUND);
        assert_eq!(create.1["code"], "workspace_not_found");
        let lifecycle = json_response(
            &fixture.router,
            authenticated(method.clone(), &command_path, None, key),
        )
        .await;
        assert_eq!(lifecycle.0, StatusCode::NOT_FOUND, "{command}");
        assert_eq!(lifecycle.1["code"], "sandbox_not_found", "{command}");
        assert_eq!(
            json_response(
                &fixture.router,
                authenticated(Method::POST, &create_path, Some(body), key),
            )
            .await,
            create,
        );
        assert_eq!(
            json_response(
                &fixture.router,
                authenticated(method, &command_path, None, key),
            )
            .await,
            lifecycle,
        );
        assert_eq!(fixture.operations.records.lock().unwrap().len(), 2);
        let changed = json_response(
            &fixture.router,
            authenticated(
                Method::POST,
                &create_path,
                Some(json!({"workspace_profile": {"id": "wsp_other", "revision": 1}})),
                key,
            ),
        )
        .await;
        assert_eq!(changed.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(changed.1["code"], "idempotency_key_reused");
    }
}
