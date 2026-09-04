use std::sync::Arc;

use super::store::{
    AgentLogStore, AgentStore, SandboxEventStore, SandboxStore, SandboxTransaction,
    SandboxTransactionStore, WorkspaceStore, WorktreeStore,
};
use super::*;
use crate::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use crate::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};

const NOW: &str = "2026-09-05T00:00:00Z";

fn sandbox_snapshot() -> SandboxProfileSnapshot {
    SandboxProfileSnapshot {
        id: SandboxProfileId("sbp_test".into()),
        revision: 1,
        digest: "a".repeat(64),
        content: SandboxProfileContent {
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
    }
}

fn workspace_snapshot() -> WorkspaceProfileSnapshot {
    WorkspaceProfileSnapshot {
        id: WorkspaceProfileId("wkp_test".into()),
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
    }
}

fn agent_snapshot() -> AgentProfileSnapshot {
    AgentProfileSnapshot {
        id: AgentProfileId("agp_test".into()),
        revision: 1,
        digest: "c".repeat(64),
        content: AgentProfileContent {
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
    }
}

fn sandbox() -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        profile: sandbox_snapshot(),
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            reaches: vec![],
            key_generation: 1,
            delivered: true,
        },
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        checkpoint: None,
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        revision: 1,
    }
}

fn workspace() -> Workspace {
    Workspace {
        id: WorkspaceId("wks_test".into()),
        sandbox_id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        profile: workspace_snapshot(),
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        status: WorkspaceStatus::Ready,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn worktree() -> Worktree {
    Worktree {
        id: WorktreeId("wkt_test".into()),
        workspace_id: WorkspaceId("wks_test".into()),
        sandbox_id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "0123456789abcdef".into(),
        status: WorktreeStatus::Ready,
        agent_id: None,
        revision: 1,
    }
}

fn agent() -> Agent {
    Agent {
        id: AgentId("agt_test".into()),
        worktree_id: WorktreeId("wkt_test".into()),
        workspace_id: WorkspaceId("wks_test".into()),
        sandbox_id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        profile: agent_snapshot(),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            conversation_id: None,
            last_seq: 0,
            first_message: None,
        },
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

#[test]
fn every_status_pair_is_governed_by_an_explicit_whitelist() {
    let sandbox_statuses = [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ];
    let expected = [
        (SandboxStatus::Provisioning, SandboxStatus::Installing),
        (SandboxStatus::Installing, SandboxStatus::Active),
        (SandboxStatus::Active, SandboxStatus::Pausing),
        (SandboxStatus::Active, SandboxStatus::Rotating),
        (SandboxStatus::Active, SandboxStatus::Retiring),
        (SandboxStatus::Pausing, SandboxStatus::Paused),
        (SandboxStatus::Paused, SandboxStatus::Reviving),
        (SandboxStatus::Paused, SandboxStatus::Retiring),
        (SandboxStatus::Reviving, SandboxStatus::Active),
        (SandboxStatus::Rotating, SandboxStatus::Active),
        (SandboxStatus::Failed, SandboxStatus::Retiring),
        (SandboxStatus::Retiring, SandboxStatus::Retired),
    ];
    for from in sandbox_statuses {
        for to in sandbox_statuses {
            let expected_allowed = expected.contains(&(from, to))
                || (!matches!(from, SandboxStatus::Failed | SandboxStatus::Retired)
                    && to == SandboxStatus::Failed);
            assert_eq!(
                from.transition(to).is_ok(),
                expected_allowed,
                "{from:?} -> {to:?}"
            );
        }
    }

    let workspace_statuses = [
        WorkspaceStatus::Preparing,
        WorkspaceStatus::Ready,
        WorkspaceStatus::Failed,
        WorkspaceStatus::Removing,
        WorkspaceStatus::Removed,
    ];
    let workspace_expected = [
        (WorkspaceStatus::Preparing, WorkspaceStatus::Ready),
        (WorkspaceStatus::Preparing, WorkspaceStatus::Failed),
        (WorkspaceStatus::Preparing, WorkspaceStatus::Removing),
        (WorkspaceStatus::Ready, WorkspaceStatus::Removing),
        (WorkspaceStatus::Failed, WorkspaceStatus::Removing),
        (WorkspaceStatus::Removing, WorkspaceStatus::Removed),
    ];
    for from in workspace_statuses {
        for to in workspace_statuses {
            assert_eq!(
                from.transition(to).is_ok(),
                workspace_expected.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }

    let worktree_statuses = [
        WorktreeStatus::Creating,
        WorktreeStatus::Ready,
        WorktreeStatus::Failed,
        WorktreeStatus::Removing,
        WorktreeStatus::Removed,
    ];
    let worktree_expected = [
        (WorktreeStatus::Creating, WorktreeStatus::Ready),
        (WorktreeStatus::Creating, WorktreeStatus::Failed),
        (WorktreeStatus::Creating, WorktreeStatus::Removing),
        (WorktreeStatus::Ready, WorktreeStatus::Removing),
        (WorktreeStatus::Failed, WorktreeStatus::Removing),
        (WorktreeStatus::Removing, WorktreeStatus::Removed),
    ];
    for from in worktree_statuses {
        for to in worktree_statuses {
            assert_eq!(
                from.transition(to).is_ok(),
                worktree_expected.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }

    let agent_statuses = [
        AgentStatus::Starting,
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
        AgentStatus::Paused,
        AgentStatus::Stopped,
        AgentStatus::Deleted,
        AgentStatus::Failed,
    ];
    let agent_expected = [
        (AgentStatus::Starting, AgentStatus::Ready),
        (AgentStatus::Starting, AgentStatus::Failed),
        (AgentStatus::Ready, AgentStatus::Working),
        (AgentStatus::Ready, AgentStatus::Paused),
        (AgentStatus::Working, AgentStatus::Ready),
        (AgentStatus::Working, AgentStatus::WaitingApproval),
        (AgentStatus::Working, AgentStatus::Paused),
        (AgentStatus::WaitingApproval, AgentStatus::Working),
        (AgentStatus::WaitingApproval, AgentStatus::Paused),
        (AgentStatus::Paused, AgentStatus::Ready),
        (AgentStatus::Stopped, AgentStatus::Deleted),
    ];
    for from in agent_statuses {
        for to in agent_statuses {
            let is_live = matches!(
                from,
                AgentStatus::Starting
                    | AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
                    | AgentStatus::Paused
            );
            let expected_allowed =
                agent_expected.contains(&(from, to)) || (is_live && to == AgentStatus::Stopped);
            assert_eq!(
                from.transition(to).is_ok(),
                expected_allowed,
                "{from:?} -> {to:?}"
            );
        }
    }
}

#[test]
fn lower_level_prerequisites_require_matching_ready_lineage() {
    let sandbox = sandbox();
    let workspace = workspace();
    let worktree = worktree();
    let agent = agent();
    workspace.validate_prerequisites(&sandbox).unwrap();
    worktree.validate_prerequisites(&workspace).unwrap();
    agent.validate_prerequisites(&worktree).unwrap();

    let mut paused = sandbox;
    paused.status = SandboxStatus::Paused;
    assert!(workspace.validate_prerequisites(&paused).is_err());
}

#[test]
fn every_aggregate_round_trips_and_refuses_unknown_fields() {
    fn round_trip<T>(value: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(serde_json::from_value::<T>(json.clone()).unwrap(), *value);
        let mut object = json.as_object().unwrap().clone();
        object.insert("unknown".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<T>(object.into()).is_err());
    }
    round_trip(&sandbox());
    round_trip(&workspace());
    round_trip(&worktree());
    round_trip(&agent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_logs_remain_gap_free_per_aggregate() {
    let store = Arc::new(InMemorySandboxStore::default());
    store.create_sandbox(sandbox()).await.unwrap();
    store.create_agent(agent()).await.unwrap();
    let mut tasks = Vec::new();
    for index in 0..100 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            store
                .append_sandbox_event(
                    &SandboxId("sbx_test".into()),
                    NOW.into(),
                    SandboxEventKind::CredentialChanged {
                        key_generation: index,
                    },
                )
                .await
                .unwrap();
            store
                .append_agent_log(
                    &AgentId("agt_test".into()),
                    NOW.into(),
                    AgentLogKind::OwnerMessage {
                        content: index.to_string(),
                    },
                )
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let sandbox_events = store
        .sandbox_events(&SandboxId("sbx_test".into()), 0, 200)
        .await
        .unwrap();
    let agent_events = store
        .agent_log(&AgentId("agt_test".into()), 0, 200)
        .await
        .unwrap();
    assert_eq!(
        sandbox_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        (1..=100).collect::<Vec<_>>()
    );
    assert_eq!(
        agent_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        (1..=100).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn worktree_and_agent_transaction_is_atomic() {
    let store = InMemorySandboxStore::default();
    store.create_sandbox(sandbox()).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    let worktree = worktree();
    let agent = agent();
    store
        .commit(SandboxTransaction::CreateWorktreeAndAgent {
            worktree: worktree.clone(),
            agent: agent.clone(),
            event_at: NOW.into(),
            event: SandboxEventKind::AgentCreated {
                workspace_id: workspace().id,
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn duplicate_resource_ids_do_not_replace_existing_records() {
    let store = InMemorySandboxStore::default();
    let original = sandbox();
    store.create_sandbox(original.clone()).await.unwrap();

    let mut duplicate = original.clone();
    duplicate.owner = SubjectId("another-owner".into());
    duplicate.name = "replacement".into();
    assert_eq!(
        store.create_sandbox(duplicate).await.unwrap_err(),
        SandboxStoreError::Duplicate
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original)
    );
}
