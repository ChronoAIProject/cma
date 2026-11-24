use std::sync::Arc;

use crate::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use crate::{
    AgentId, AgentProfileId, Authorization, Caller, Capability, SandboxId, SandboxProfileId,
    SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId,
};

use super::*;
#[path = "ledger_retirement_tests.rs"]
mod ledger_retirement;
mod shared;

const NOW: &str = "2026-09-05T00:00:00Z";

fn caller(subject: &str, allowed: bool) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new(allowed.then_some(Capability::UseOwnSandboxes)),
    )
}

fn sandbox(id: &str, owner: &str, status: SandboxStatus) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("sandbox-{id}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_hierarchy".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
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
            max_workspaces: 4,
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

fn workspace(parent: &Sandbox, id: &str, owner: Option<&str>) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: parent.id.clone(),
        owner: owner.map_or_else(|| parent.owner.clone(), SubjectId::new),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId(format!("wkp_{id}")),
            revision: 2,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: id.into() },
                name: id.into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: id.into(),
        path: format!("/home/cma/{id}"),
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

fn worktree(parent: &Workspace, id: &str, agent_id: &str) -> Worktree {
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

fn agent(parent: &Workspace, worktree: &Worktree, id: &str) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: worktree.id.clone(),
        workspace_id: parent.id.clone(),
        sandbox_id: parent.sandbox_id.clone(),
        owner: parent.owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_hierarchy".into()),
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
        capabilities: crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]),
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
        title: Some("Read hierarchy".into()),
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

async fn insert_sandbox(store: &InMemorySandboxStore, value: Sandbox) {
    store.create_sandbox(value).await.unwrap();
}

#[tokio::test]
async fn status_is_filtered_before_owner_pagination() {
    let store = Arc::new(InMemorySandboxStore::default());
    for value in [
        sandbox("sbx_1", "owner", SandboxStatus::Active),
        sandbox("sbx_2", "owner", SandboxStatus::Paused),
        sandbox("sbx_3", "owner", SandboxStatus::Active),
        sandbox("sbx_0", "foreign", SandboxStatus::Active),
    ] {
        insert_sandbox(&store, value).await;
    }
    let service = HierarchyReadService::new(store);
    let first = service
        .list_sandboxes(
            &caller("owner", true),
            &OwnerSandboxQuery {
                exclude_retired: false,
                status: Some(SandboxStatus::Active),
                page: PageRequest::new(1, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items[0].sandbox.id.as_str(), "sbx_1");
    assert_eq!(first.next.as_deref(), Some("sbx_1"));

    let second = service
        .list_sandboxes(
            &caller("owner", true),
            &OwnerSandboxQuery {
                exclude_retired: false,
                status: Some(SandboxStatus::Active),
                page: PageRequest::new(1, first.next).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items[0].sandbox.id.as_str(), "sbx_3");
    assert_eq!(second.next, None);
}

#[tokio::test]
async fn detail_projects_sorted_summaries_and_exact_counts() {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_parent", "owner", SandboxStatus::Active);
    insert_sandbox(&store, parent.clone()).await;
    let later = workspace(&parent, "wks_z", None);
    let earlier = workspace(&parent, "wks_a", None);
    store.create_workspace(later.clone()).await.unwrap();
    store.create_workspace(earlier.clone()).await.unwrap();
    let tree = worktree(&later, "wkt_z", "agt_z");
    store.create_worktree(tree.clone()).await.unwrap();
    store
        .create_agent(agent(&later, &tree, "agt_z"))
        .await
        .unwrap();

    let service = HierarchyReadService::new(store);
    let detail = service
        .sandbox(&caller("owner", true), &parent.id)
        .await
        .unwrap();
    assert_eq!(
        detail.counts,
        HierarchyCounts {
            workspaces: 2,
            worktrees: 1,
            agents: 1,
        }
    );
    assert_eq!(detail.workspaces[0].id, earlier.id);
    assert_eq!(detail.workspaces[1].counts.worktrees, 1);
    assert_eq!(detail.workspaces[1].counts.agents, 1);

    let workspace = service
        .workspace(&caller("owner", true), &later.id)
        .await
        .unwrap();
    assert_eq!(workspace.counts.worktrees, 1);
    assert_eq!(workspace.counts.agents, 1);
    assert_eq!(
        workspace.worktrees[0].agent_id.as_ref().unwrap().as_str(),
        "agt_z"
    );
    assert_eq!(workspace.agents[0].profile_revision, 3);
}

#[tokio::test]
async fn every_foreign_and_missing_identifier_is_the_same_not_found() {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_foreign", "foreign", SandboxStatus::Active);
    insert_sandbox(&store, parent.clone()).await;
    let workspace = workspace(&parent, "wks_foreign", None);
    store.create_workspace(workspace.clone()).await.unwrap();
    let tree = worktree(&workspace, "wkt_foreign", "agt_foreign");
    store.create_worktree(tree.clone()).await.unwrap();
    let service = HierarchyReadService::new(store);
    let owner = caller("owner", true);

    assert_eq!(
        service.sandbox(&owner, &parent.id).await,
        Err(HierarchyReadError::NotFound)
    );
    assert_eq!(
        service
            .sandbox(&owner, &SandboxId("sbx_missing".into()))
            .await,
        Err(HierarchyReadError::NotFound)
    );
    assert_eq!(
        service.workspace(&owner, &workspace.id).await,
        Err(HierarchyReadError::NotFound)
    );
    assert_eq!(
        service
            .workspace(&owner, &WorkspaceId("wks_missing".into()))
            .await,
        Err(HierarchyReadError::NotFound)
    );
    assert_eq!(
        service.worktree(&owner, &tree.id).await,
        Err(HierarchyReadError::NotFound)
    );
    assert_eq!(
        service
            .worktree(&owner, &WorktreeId("wkt_missing".into()))
            .await,
        Err(HierarchyReadError::NotFound)
    );
}

#[tokio::test]
async fn authorization_and_child_lineage_fail_closed() {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_parent", "owner", SandboxStatus::Active);
    insert_sandbox(&store, parent.clone()).await;
    store
        .create_workspace(workspace(&parent, "wks_bad", Some("foreign")))
        .await
        .unwrap();
    let service = HierarchyReadService::new(store);

    assert_eq!(
        service.sandbox(&caller("owner", false), &parent.id).await,
        Err(HierarchyReadError::Forbidden)
    );
    assert!(matches!(
        service.sandbox(&caller("owner", true), &parent.id).await,
        Err(HierarchyReadError::Corrupt(_))
    ));
}

#[tokio::test]
async fn visible_inventory_excludes_only_retired_records_before_pagination() {
    let store = Arc::new(InMemorySandboxStore::default());
    for (id, owner, status) in [
        ("sbx_00", "owner", SandboxStatus::Retired),
        ("sbx_01", "owner", SandboxStatus::Active),
        ("sbx_02", "owner", SandboxStatus::Retired),
        ("sbx_03", "owner", SandboxStatus::Retiring),
        ("sbx_04", "foreign", SandboxStatus::Active),
        ("sbx_05", "owner", SandboxStatus::Paused),
        ("sbx_06", "owner", SandboxStatus::Retired),
    ] {
        insert_sandbox(&store, sandbox(id, owner, status)).await;
    }
    let service = HierarchyReadService::new(store);
    let owner = caller("owner", true);
    let mut query = OwnerSandboxQuery {
        exclude_retired: true,
        page: PageRequest::new(1, None).unwrap(),
        ..Default::default()
    };
    for (id, next) in [
        ("sbx_01", Some("sbx_01")),
        ("sbx_03", Some("sbx_03")),
        ("sbx_05", None),
    ] {
        let page = service.list_sandboxes(&owner, &query).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].sandbox.id.as_str(), id);
        assert_eq!(page.next.as_deref(), next);
        query.page.cursor = page.next;
    }
    let retained = service
        .list_sandboxes(&owner, &OwnerSandboxQuery::default())
        .await
        .unwrap();
    assert_eq!(retained.items.len(), 6);
    assert_eq!(
        service
            .sandbox(&owner, &SandboxId("sbx_00".into()))
            .await
            .unwrap()
            .sandbox
            .status,
        SandboxStatus::Retired
    );
    query.status = Some(SandboxStatus::Retired);
    let empty = service.list_sandboxes(&owner, &query).await.unwrap();
    assert!(empty.items.is_empty());
    assert_eq!(empty.next, None);
}
