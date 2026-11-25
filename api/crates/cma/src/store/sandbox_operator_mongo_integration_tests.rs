use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use layer2_core::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentStore, PageRequest, SandboxEventStore, SandboxStore, SandboxStoreError, WorkspaceStore,
};
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
    RuntimeBinding, Sandbox, SandboxAdminQuery, SandboxAdminStore, SandboxCheckpointRef,
    SandboxCredentialState, SandboxEventKind, SandboxLimits, SandboxOrigin, SandboxRetirement,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep, StoredSandboxRetirement,
    TypedFailure, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep,
};
use layer3_adapter::{CheckpointReceipt, WriterState};
use mongodb::Client;

use super::sandbox_mongo::MongoSandboxStore;

const NOW: &str = "2026-09-05T00:00:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) async fn database(prefix: &str) -> (Client, String, MongoSandboxStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_814_sandbox_{prefix}_{}_{sequence}", std::process::id());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (client, name, store)
}

pub(super) fn sandbox(
    id: &str,
    owner: &str,
    name: &str,
    status: SandboxStatus,
    runtime_version: &str,
) -> Sandbox {
    let mut value = Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId(owner.into()),
        name: name.into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_operator".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status,
        failure: None,
        runtime: Some(RuntimeBinding {
            boot_id: None,
            runtime_id: format!("runtime-{id}"),
            runtime_version: runtime_version.into(),
            provision_generation: Some(1),
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
    };
    if status == SandboxStatus::Paused {
        value.runtime = None;
        value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
        value.lifecycle.checkpoints.latest = Some(
            SandboxCheckpointRef::seal(
                &CheckpointReceipt {
                    epoch: 1,
                    object_reference: format!("checkpoints/{id}/1"),
                    sha256: "b".repeat(64),
                    writers: WriterState::Held,
                },
                &[],
                NOW,
            )
            .unwrap(),
        );
    }
    value
}

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace {
        id: WorkspaceId(format!("wks_{}", sandbox.id.as_str())),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_operator".into()),
            revision: 1,
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
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

pub(super) fn agent(sandbox: &Sandbox, id: &str, status: AgentStatus) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        workspace_id: WorkspaceId(format!("wks_{}", sandbox.id.as_str())),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_operator".into()),
            revision: 1,
            digest: "c".repeat(64),
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
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some(format!("conversation-{id}")),
            runtime_generation: Some(1),
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: (status == AgentStatus::Failed).then(|| TypedFailure {
            class: "startup_failed".into(),
            detail: "startup failed".into(),
        }),
        title: Some(format!("Agent {id}")),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: (status == AgentStatus::Deleted).then(|| NOW.into()),
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

fn query(limit: u16, cursor: Option<String>) -> SandboxAdminQuery {
    SandboxAdminQuery {
        page: PageRequest { limit, cursor },
        ..SandboxAdminQuery::default()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_operator_inventory_filters_pages_counts_and_reads_details() {
    let (client, name, store) = database("inventory").await;
    let first = sandbox(
        "sbx_admin_a",
        "owner-a",
        "alpha",
        SandboxStatus::Active,
        "v1",
    );
    let second = sandbox(
        "sbx_admin_b",
        "owner-b",
        "beta",
        SandboxStatus::Paused,
        "v2",
    );
    let third = sandbox(
        "sbx_admin_c",
        "owner-a",
        "gamma",
        SandboxStatus::Paused,
        "v1",
    );
    for value in [&first, &second, &third] {
        store.create_sandbox(value.clone()).await.unwrap();
    }
    store.create_workspace(workspace(&first)).await.unwrap();
    store
        .create_agent(agent(&first, "agt_admin_live", AgentStatus::Working))
        .await
        .unwrap();
    store
        .create_agent(agent(&first, "agt_admin_deleted", AgentStatus::Deleted))
        .await
        .unwrap();

    let page = store.list_admin_sandboxes(&query(1, None)).await.unwrap();
    assert_eq!(page.items[0].id, first.id);
    assert_eq!(page.items[0].agent_count, 1);
    assert_eq!(page.next.as_deref(), Some("sbx_admin_a"));
    let page = store
        .list_admin_sandboxes(&query(1, page.next))
        .await
        .unwrap();
    assert_eq!(page.items[0].id, second.id);
    assert_eq!(page.next.as_deref(), Some("sbx_admin_b"));

    let mut filtered = query(10, None);
    filtered.owner = Some(SubjectId("owner-a".into()));
    assert_eq!(
        store
            .list_admin_sandboxes(&filtered)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    filtered.status = Some(SandboxStatus::Paused);
    assert_eq!(
        store.list_admin_sandboxes(&filtered).await.unwrap().items[0].id,
        third.id
    );
    filtered.status = None;
    filtered.runtime_version = Some("v1".into());
    assert_eq!(
        store
            .list_admin_sandboxes(&filtered)
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    let view = store.read_admin_sandbox(&first.id).await.unwrap().unwrap();
    assert_eq!(view.workspaces.len(), 1);
    assert_eq!(view.workspaces[0].profile_id, "wkp_operator");
    assert_eq!(view.agents.len(), 2);
    assert_eq!(view.agents[0].id.as_str(), "agt_admin_deleted");
    let event = store
        .append_sandbox_event(&first.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!(
        store
            .read_admin_sandbox_events(&first.id, 0, 10)
            .await
            .unwrap(),
        vec![event]
    );
    assert_eq!(
        store
            .read_admin_sandbox_events(&SandboxId("sbx_missing".into()), 0, 10)
            .await,
        Err(SandboxStoreError::NotFound)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_retirement_uses_the_shared_transaction_and_leaves_agents_for_teardown() {
    let (client, name, store) = database("retirement").await;
    let mut target = sandbox(
        "sbx_retire",
        "another-owner",
        "retire",
        SandboxStatus::Active,
        "v1",
    );
    store.create_sandbox(target.clone()).await.unwrap();
    let working = agent(&target, "agt_still_working", AgentStatus::Working);
    store.create_agent(working.clone()).await.unwrap();
    let retirement = StoredSandboxRetirement::new(Arc::new(store.clone()));
    let observed_revision = target.revision;
    target.last_activity_at = "2026-09-06T00:00:00Z".into();
    target.revision += 1;
    store
        .save_sandbox(observed_revision, target.clone())
        .await
        .unwrap();
    assert!(matches!(
        retirement
            .retire(&target.id, NOW.into(), Some(observed_revision))
            .await,
        Err(layer2_core::sandbox::OperatorError::InvalidState(_))
    ));
    assert_eq!(
        store.get_sandbox(&target.id).await.unwrap(),
        Some(target.clone())
    );
    assert!(store
        .sandbox_events(&target.id, 0, 10)
        .await
        .unwrap()
        .is_empty());

    let result = retirement
        .retire(&target.id, NOW.into(), Some(target.revision))
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.sandbox.status, SandboxStatus::Retiring);
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );
    let events = store.sandbox_events(&target.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Retiring,
        }
    );
    let repeated = retirement
        .retire(&target.id, NOW.into(), None)
        .await
        .unwrap();
    assert!(!repeated.changed);
    assert_eq!(
        store.sandbox_events(&target.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "workspace_prepare_recovery_mongo_tests.rs"]
mod prepare_recovery;
