use std::sync::Arc;

use crate::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use crate::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentStore, SandboxStore,
    SandboxStoreError, WorkspaceStore,
};
use crate::sandbox::{
    AgentOrigin, AgentStatus, Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep, Workspace,
    WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};

use super::InMemorySandboxStore;

mod bootstrap;
mod channel_usage;
mod creation_revival;
mod provenance;
mod provenance_backfill;
mod provenance_inventory;
mod sandbox_controls_tests;

pub(super) const NOW: &str = "2026-09-05T00:00:00Z";

fn sandbox(limit: u8) -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_agent_admission".into()),
        owner: SubjectId::new("owner"),
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_agent_admission".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: limit,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
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
            max_workspaces: 1,
            max_agents_per_workspace: limit,
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

fn workspace() -> Workspace {
    Workspace {
        id: WorkspaceId("wks_agent_admission".into()),
        sandbox_id: SandboxId("sbx_agent_admission".into()),
        owner: SubjectId::new("owner"),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_agent_admission".into()),
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
        resolved_commit: Some("a".repeat(40)),
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

fn profile() -> AgentProfileSnapshot {
    AgentProfileSnapshot {
        id: AgentProfileId("agp_agent_admission".into()),
        revision: 3,
        digest: "c".repeat(64),
        content: AgentProfileContent {
            isolated_agent: false,
            model: Some("model-a".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "Be exact.".into(),
        },
    }
}

pub(super) fn admission(key: &str) -> AgentAdmission {
    AgentAdmission {
        binding: None,
        created_by: None,
        owner: SubjectId::new("owner"),
        workspace_id: WorkspaceId("wks_agent_admission".into()),
        agent_id: AgentId::generate(),
        worktree_id: WorktreeId::generate(),
        profile: profile(),
        capabilities: crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: key.into(),
        event_at: NOW.into(),
    }
}

pub(super) async fn ready_store(limit: u8) -> InMemorySandboxStore {
    let store = InMemorySandboxStore::default();
    store.create_sandbox(sandbox(limit)).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store
}

pub(super) fn created(result: AgentAdmissionResult) -> crate::sandbox::store::AgentCreation {
    match result {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("expected a fresh admission"),
    }
}

#[tokio::test]
async fn concurrent_admissions_at_limit_minus_one_admit_exactly_one() {
    let store = Arc::new(ready_store(2).await);
    store.admit_agent(admission("first")).await.unwrap();

    let left = store.admit_agent(admission("left"));
    let right = store.admit_agent(admission("right"));
    let results = tokio::join!(left, right);
    let values = [results.0, results.1];

    assert_eq!(
        values
            .iter()
            .filter(|result| matches!(result, Ok(AgentAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        values
            .iter()
            .filter(|result| matches!(
                result,
                Err(SandboxStoreError::AgentLimitReached { limit: 2, live: 2 })
            ))
            .count(),
        1
    );
    let state = store.lock();
    assert_eq!(state.agents.len(), 2);
    assert_eq!(state.worktrees.len(), 2);
    assert_eq!(state.agent_logs.len(), 2);
    assert_eq!(state.workspaces[&workspace().id].next_worktree_number, 3);
}

#[tokio::test]
async fn allocations_have_distinct_monotonic_names_branches_and_paths() {
    let store = ready_store(4).await;
    let first = created(store.admit_agent(admission("first")).await.unwrap());
    let second = created(store.admit_agent(admission("second")).await.unwrap());
    let third = created(store.admit_agent(admission("third")).await.unwrap());

    assert_eq!(
        [
            first.worktree.name,
            second.worktree.name,
            third.worktree.name
        ],
        ["wt-1", "wt-2", "wt-3"]
    );
    assert_eq!(second.worktree.branch, "cma/wt-2");
    assert_eq!(third.worktree.path, "/home/cma/project/.worktrees/wt-3");
    assert_eq!(third.worktree.base_commit, "a".repeat(40));
}

#[tokio::test]
async fn failed_and_deleted_allocations_never_release_their_suffixes() {
    let store = ready_store(3).await;
    let first = created(store.admit_agent(admission("first")).await.unwrap());
    {
        let mut state = store.lock();
        state.agents.get_mut(&first.agent.id).unwrap().status = AgentStatus::Failed;
    }
    let second = created(store.admit_agent(admission("second")).await.unwrap());
    {
        let mut state = store.lock();
        state.agents.get_mut(&second.agent.id).unwrap().status = AgentStatus::Deleted;
    }
    let third = created(store.admit_agent(admission("third")).await.unwrap());

    assert_eq!(first.worktree.name, "wt-1");
    assert_eq!(second.worktree.name, "wt-2");
    assert_eq!(third.worktree.name, "wt-3");
}

#[tokio::test]
async fn exact_creation_key_replay_returns_existing_and_changed_payload_conflicts() {
    let store = ready_store(2).await;
    let request = admission("stable-key");
    let first = created(store.admit_agent(request.clone()).await.unwrap());
    assert_eq!(
        store.admit_agent(request.clone()).await.unwrap(),
        AgentAdmissionResult::Existing(first)
    );
    {
        let mut state = store.lock();
        let stored = state
            .agents
            .values_mut()
            .find(|agent| agent.creation_key == "stable-key")
            .unwrap();
        stored.title = Some("generated later".into());
    }
    assert!(matches!(
        store.admit_agent(request.clone()).await.unwrap(),
        AgentAdmissionResult::Existing(_)
    ));

    let mut changed_grants = request.clone();
    changed_grants.capabilities = crate::RuntimeCapabilities::new([
        crate::RuntimeCapability::Conversation,
        crate::RuntimeCapability::Resources,
    ]);
    assert_eq!(
        store.admit_agent(changed_grants).await.unwrap_err(),
        SandboxStoreError::AgentCreationConflict
    );

    let mut changed = request;
    changed.title = Some("different".into());
    assert_eq!(
        store.admit_agent(changed).await.unwrap_err(),
        SandboxStoreError::AgentCreationConflict
    );
}

#[tokio::test]
async fn replay_fails_closed_when_a_creation_artifact_is_cross_linked() {
    let store = ready_store(2).await;
    let request = admission("corrupt-replay");
    let creation = created(store.admit_agent(request.clone()).await.unwrap());
    {
        let mut state = store.lock();
        state
            .worktrees
            .get_mut(&creation.worktree.id)
            .unwrap()
            .agent_id = Some(AgentId("agt_someone_else".into()));
    }
    assert!(matches!(
        store.admit_agent(request).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
}

#[tokio::test]
async fn late_failure_rolls_back_every_admission_artifact() {
    let store = ready_store(2).await;
    {
        let mut state = store.lock();
        state
            .sandboxes
            .get_mut(&SandboxId("sbx_agent_admission".into()))
            .unwrap()
            .revision = u64::MAX;
    }
    let before = {
        let state = store.lock();
        (
            state.workspaces[&workspace().id].clone(),
            state.sandboxes[&sandbox(2).id].clone(),
            state.sandbox_events.clone(),
            state.worktrees.len(),
            state.agents.len(),
            state.agent_logs.len(),
        )
    };

    assert_eq!(
        store.admit_agent(admission("will-roll-back")).await,
        Err(SandboxStoreError::Conflict)
    );
    let state = store.lock();
    assert_eq!(state.workspaces[&workspace().id], before.0);
    assert_eq!(state.sandboxes[&sandbox(2).id], before.1);
    assert_eq!(state.sandbox_events, before.2);
    assert_eq!(state.worktrees.len(), before.3);
    assert_eq!(state.agents.len(), before.4);
    assert_eq!(state.agent_logs.len(), before.5);
}

#[tokio::test]
async fn atomic_recheck_distinguishes_workspace_and_sandbox_readiness() {
    let store = ready_store(1).await;
    {
        let mut state = store.lock();
        state.workspaces.get_mut(&workspace().id).unwrap().status = WorkspaceStatus::Preparing;
    }
    assert_eq!(
        store.admit_agent(admission("workspace-not-ready")).await,
        Err(SandboxStoreError::WorkspaceNotReady)
    );
    {
        let mut state = store.lock();
        state.workspaces.get_mut(&workspace().id).unwrap().status = WorkspaceStatus::Ready;
        state.sandboxes.get_mut(&sandbox(1).id).unwrap().status = SandboxStatus::Installing;
    }
    assert_eq!(
        store.admit_agent(admission("sandbox-not-ready")).await,
        Err(SandboxStoreError::SandboxNotReady)
    );
    assert!(store
        .get_agent(&AgentId("missing".into()))
        .await
        .unwrap()
        .is_none());
}
