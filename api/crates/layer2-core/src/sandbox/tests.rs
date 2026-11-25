use std::sync::Arc;

#[path = "answer_deliverables_tests.rs"]
mod answer_deliverables;
#[path = "lifecycle_transition_contract_tests.rs"]
mod transition_contract;

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
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
    }
}

pub(super) fn sandbox() -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: sandbox_snapshot(),
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 1,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: "provider-key".into(),
                provider_state_version: 1,
                issued_at: NOW.into(),
                expires_at: "2026-10-05T00:00:00Z".into(),
                grant_fingerprint: "a".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
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

pub(super) fn workspace() -> Workspace {
    Workspace {
        id: WorkspaceId("wks_test".into()),
        sandbox_id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        origin: WorkspaceOrigin::Manual,
        profile: workspace_snapshot(),
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

pub(super) fn worktree() -> Worktree {
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
        agent_id: Some(AgentId("agt_test".into())),
        revision: 1,
    }
}

pub(super) fn agent() -> Agent {
    Agent {
        id: AgentId("agt_test".into()),
        worktree_id: WorktreeId("wkt_test".into()),
        workspace_id: WorkspaceId("wks_test".into()),
        sandbox_id: SandboxId("sbx_test".into()),
        owner: SubjectId("owner".into()),
        creation_key: "create-test-agent".into(),
        creation_fingerprint: "d".repeat(64),
        profile: agent_snapshot(),
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
        work: AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into()),
        failure: None,
        title: None,
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

#[test]
fn legacy_agent_without_grants_deserializes_fail_closed() {
    let mut value = serde_json::to_value(agent()).unwrap();
    value.as_object_mut().unwrap().remove("capabilities");
    let restored: Agent = serde_json::from_value(value).unwrap();
    assert_eq!(restored.capabilities, crate::RuntimeCapabilities::default());
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
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ];
    let expected = [
        (SandboxStatus::Provisioning, SandboxStatus::Installing),
        (SandboxStatus::Provisioning, SandboxStatus::Reviving),
        (SandboxStatus::Provisioning, SandboxStatus::Retiring),
        (SandboxStatus::Installing, SandboxStatus::Active),
        (SandboxStatus::Installing, SandboxStatus::Reviving),
        (SandboxStatus::Installing, SandboxStatus::Retiring),
        (SandboxStatus::Active, SandboxStatus::Pausing),
        (SandboxStatus::Active, SandboxStatus::Reviving),
        (SandboxStatus::Active, SandboxStatus::CredentialRotating),
        (SandboxStatus::Active, SandboxStatus::Rotating),
        (SandboxStatus::Active, SandboxStatus::Retiring),
        (SandboxStatus::Pausing, SandboxStatus::Paused),
        (SandboxStatus::Pausing, SandboxStatus::Reviving),
        (SandboxStatus::Pausing, SandboxStatus::Active),
        (SandboxStatus::Pausing, SandboxStatus::Retiring),
        (SandboxStatus::Paused, SandboxStatus::Reviving),
        (SandboxStatus::Paused, SandboxStatus::Rotating),
        (SandboxStatus::Paused, SandboxStatus::Retiring),
        (SandboxStatus::Reviving, SandboxStatus::Active),
        (SandboxStatus::Reviving, SandboxStatus::Pausing),
        (SandboxStatus::Reviving, SandboxStatus::Retiring),
        (SandboxStatus::CredentialRotating, SandboxStatus::Active),
        (SandboxStatus::CredentialRotating, SandboxStatus::Reviving),
        (SandboxStatus::CredentialRotating, SandboxStatus::Retiring),
        (SandboxStatus::Rotating, SandboxStatus::Active),
        (SandboxStatus::Rotating, SandboxStatus::Pausing),
        (SandboxStatus::Rotating, SandboxStatus::Paused),
        (SandboxStatus::Rotating, SandboxStatus::Retiring),
        (SandboxStatus::Failed, SandboxStatus::Retiring),
        (SandboxStatus::Retiring, SandboxStatus::Retired),
    ];
    for from in sandbox_statuses {
        for to in sandbox_statuses {
            let expected_allowed = expected.contains(&(from, to))
                || (!matches!(from, SandboxStatus::Failed | SandboxStatus::Retired)
                    && to == SandboxStatus::Failed)
                || (!matches!(from, SandboxStatus::Retiring | SandboxStatus::Retired)
                    && to == SandboxStatus::Retiring);
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
        (WorkspaceStatus::Removing, WorkspaceStatus::Failed),
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
        (AgentStatus::WaitingApproval, AgentStatus::Ready),
        (AgentStatus::WaitingApproval, AgentStatus::Paused),
        (AgentStatus::Paused, AgentStatus::Ready),
        (AgentStatus::Stopped, AgentStatus::Deleted),
        (AgentStatus::Failed, AgentStatus::Deleted),
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
fn revision_overflow_leaves_aggregates_unchanged() {
    let mut sandbox = sandbox();
    sandbox.status = SandboxStatus::Provisioning;
    sandbox.revision = u64::MAX;
    let original = sandbox.clone();
    assert!(sandbox
        .transition(SandboxStatus::Installing, "later".into(), None)
        .is_err());
    assert_eq!(sandbox, original);

    let mut workspace = workspace();
    workspace.status = WorkspaceStatus::Preparing;
    workspace.ready_at = None;
    workspace.revision = u64::MAX;
    let original = workspace.clone();
    assert!(workspace
        .transition(WorkspaceStatus::Ready, "later".into(), None)
        .is_err());
    assert_eq!(workspace, original);

    let mut worktree = worktree();
    worktree.status = WorktreeStatus::Creating;
    worktree.revision = u64::MAX;
    let original = worktree.clone();
    assert!(worktree.transition(WorktreeStatus::Ready).is_err());
    assert_eq!(worktree, original);

    let mut agent = agent();
    agent.revision = u64::MAX;
    let original = agent.clone();
    assert!(agent
        .transition(AgentStatus::Ready, "later".into(), None)
        .is_err());
    assert_eq!(agent, original);
}

#[test]
fn agent_control_model_schedules_stop_intent_before_terminal_publication() {
    let mut agent = agent();
    let fence = agent
        .work
        .claim("old-worker".into(), NOW, "2026-09-05T00:01:00Z".into())
        .unwrap();
    assert!(agent.work.owns("old-worker", fence));
    agent
        .request_stop(AgentStopReason::Owner, "2026-09-05T00:00:30Z".into())
        .unwrap();
    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(agent.requested_stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(agent.stop_reason, None);
    assert_eq!(agent.stopped_at, None);
    assert_eq!(agent.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(agent.work.ready_at, "2026-09-05T00:00:30Z");
    assert_eq!(
        agent.work.cleanup_not_before.as_deref(),
        Some("2026-09-05T00:01:00Z")
    );
    assert!(agent.work.lease.is_none());

    let fence = agent
        .work
        .claim(
            "stop-worker".into(),
            "2026-09-05T00:00:30Z",
            "2026-09-05T00:01:30Z".into(),
        )
        .unwrap();
    agent
        .work
        .advance(
            "stop-worker",
            fence,
            AgentWorkStep::CloseProcess,
            "2026-09-05T00:00:30Z".into(),
        )
        .unwrap();
    assert_eq!(agent.work.ready_at, "2026-09-05T00:01:00Z");
    assert_eq!(agent.work.cleanup_not_before, None);

    let scheduled = agent.clone();
    assert!(agent
        .transition(
            AgentStatus::Stopped,
            "2026-09-05T00:01:00Z".into(),
            Some(AgentStopReason::Operator),
        )
        .is_err());
    assert_eq!(agent, scheduled);
    agent
        .transition(
            AgentStatus::Stopped,
            "2026-09-05T00:01:00Z".into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    assert_eq!(agent.requested_stop_reason, None);
    assert_eq!(agent.stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(agent.stopped_at.as_deref(), Some("2026-09-05T00:01:00Z"));
}

#[test]
fn agent_control_model_schedules_delete_without_publishing_deleted() {
    let mut stopped = agent();
    stopped.status = AgentStatus::Stopped;
    stopped.stopped_at = Some(NOW.into());
    stopped.stop_reason = Some(AgentStopReason::Owner);
    stopped.work.schedule(AgentWorkStep::None, NOW.into());
    let original_revision = stopped.revision;
    stopped
        .request_delete("2026-09-05T00:01:00Z".into())
        .unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.deleted_at, None);
    assert_eq!(stopped.work.step, AgentWorkStep::EnsureProcessClosed);
    assert_eq!(stopped.revision, original_revision + 1);

    let mut live = agent();
    let original = live.clone();
    assert!(live.request_delete(NOW.into()).is_err());
    assert_eq!(live, original);
}

#[test]
fn agent_control_model_revision_overflow_is_atomic() {
    let mut stopping = agent();
    stopping.revision = u64::MAX;
    let original = stopping.clone();
    assert!(stopping
        .request_stop(AgentStopReason::Owner, NOW.into())
        .is_err());
    assert_eq!(stopping, original);

    let mut deleting = agent();
    deleting.status = AgentStatus::Failed;
    deleting.revision = u64::MAX;
    let original = deleting.clone();
    assert!(deleting.request_delete(NOW.into()).is_err());
    assert_eq!(deleting, original);
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
            worktree: Box::new(worktree.clone()),
            agent: Box::new(agent.clone()),
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

#[test]
fn workspace_limits_count_only_live_children_of_the_sandbox() {
    let mut sandbox = sandbox();
    sandbox.limits.max_workspaces = 1;
    let live = workspace();
    assert_eq!(
        enforce_workspace_limit(&sandbox, std::slice::from_ref(&live)),
        Err(SandboxLimitError::WorkspaceLimitReached { limit: 1, live: 1 })
    );

    let mut removed = live.clone();
    removed.status = WorkspaceStatus::Removed;
    removed.removed_at = Some(NOW.into());
    let mut foreign = live;
    foreign.id = WorkspaceId("wks_foreign".into());
    foreign.sandbox_id = SandboxId("sbx_foreign".into());
    assert_eq!(
        enforce_workspace_limit(&sandbox, &[removed, foreign]),
        Ok(())
    );
}

#[test]
fn agent_limits_count_only_live_agents_in_the_selected_workspace() {
    let mut sandbox = sandbox();
    sandbox.limits.max_agents_per_workspace = 2;
    let first = agent();
    let mut second = first.clone();
    second.id = AgentId("agt_second".into());
    second.status = AgentStatus::WaitingApproval;
    assert_eq!(
        enforce_agent_limit(&sandbox, &first.workspace_id, &[first.clone(), second]),
        Err(SandboxLimitError::AgentLimitReached { limit: 2, live: 2 })
    );

    let mut stopped = first.clone();
    stopped.status = AgentStatus::Stopped;
    stopped.stopped_at = Some(NOW.into());
    let mut deleted = first.clone();
    deleted.id = AgentId("agt_deleted".into());
    deleted.status = AgentStatus::Deleted;
    deleted.deleted_at = Some(NOW.into());
    let mut other_workspace = first;
    other_workspace.id = AgentId("agt_other".into());
    other_workspace.workspace_id = WorkspaceId("wks_other".into());
    assert_eq!(
        enforce_agent_limit(
            &sandbox,
            &WorkspaceId("wks_test".into()),
            &[stopped, deleted, other_workspace],
        ),
        Ok(())
    );
}

#[path = "agent_attachments_tests.rs"]
mod attachments;

#[path = "agent_filesystem_tests.rs"]
mod filesystem;

#[path = "desktop_tests.rs"]
mod desktop;
