use super::*;
use crate::sandbox::agent_skills::EmptyAgentSkillResolver;
use crate::sandbox::agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies};
use layer2_core::profiles::{WorkspaceProfileContent, WorkspaceProfileSnapshot};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionStore, AgentControlStore, AgentControlTarget, AgentStopRequest,
    WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionStore, WorkspaceStore,
};
use layer2_core::sandbox::{
    AgentOrigin, FirstMessageDelivery, Workspace, WorkspaceOrigin, WorkspaceSource,
    WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};

#[tokio::test]
async fn paused_create_survives_each_revive_restart_and_starts_after_checkpoint_agents() {
    // This scenario retains several complete generations across its awaits.
    // Keep the test's aggregate future off the small default test-thread stack.
    Box::pin(paused_create_scenario()).await;
}

async fn paused_create_scenario() {
    let harness = Harness::new(vec!["install-once".into()], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let previous = prepare_agent(&harness, &active, now, 1).await;
    let workspace = workspace_for(&previous, now);
    harness
        .store
        .create_workspace(workspace.clone())
        .await
        .unwrap();
    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, pause_at).await;
    }
    assert_eq!(
        harness.stored(&active.id).await.status,
        SandboxStatus::Paused
    );
    let revive_at = pause_at + Duration::from_secs(1);
    let request = AgentAdmission {
        binding: None,
        created_by: None,
        owner: active.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: layer2_core::AgentId::generate(),
        worktree_id: layer2_core::WorktreeId::generate(),
        profile: previous.profile.clone(),
        capabilities: previous.capabilities.clone(),
        title: None,
        first_message: Some(
            FirstMessageRecord::new(
                "paused-create",
                "Inspect the restored project",
                rfc3339_at(revive_at),
            )
            .unwrap(),
        ),
        origin: AgentOrigin::Manual,
        creation_key: "paused-create".into(),
        event_at: rfc3339_at(revive_at),
    };
    let new_agent = harness
        .store
        .admit_agent(request.clone())
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    let mut cancelled_request = request.clone();
    cancelled_request.agent_id = layer2_core::AgentId::generate();
    cancelled_request.worktree_id = layer2_core::WorktreeId::generate();
    cancelled_request.creation_key = "cancelled-paused-create".into();
    cancelled_request.first_message = None;
    let cancelled = harness
        .store
        .admit_agent(cancelled_request)
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    harness
        .store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&cancelled),
            reason: layer2_core::sandbox::AgentStopReason::Owner,
            requested_at: rfc3339_at(revive_at),
        })
        .await
        .unwrap();
    let registry = Arc::new(LiveAgentRegistry::default());
    let mut config = AgentWorkerConfig::new("new-agent-worker", "test");
    config.retry_delay = Duration::from_secs(1);
    let agent_worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), registry.clone()),
            registry,
        },
        config,
    );
    agent_worker
        .run_one(&new_agent.id, revive_at)
        .await
        .unwrap();
    for _ in 0..2 {
        agent_worker
            .run_one(&cancelled.id, revive_at)
            .await
            .unwrap();
    }
    assert!(harness
        .store
        .get_agent(&new_agent.id)
        .await
        .unwrap()
        .unwrap()
        .work
        .cleanup_debt
        .is_empty());
    for _ in 0..8 {
        restarted_worker(&harness, revive_at)
            .await
            .run_one(&active.id, revive_at)
            .await
            .unwrap();
        let replay = harness.store.admit_agent(request.clone()).await.unwrap();
        assert_eq!(replay.creation().agent.id, new_agent.id);
    }
    let revived = harness.stored(&active.id).await;
    assert_eq!(revived.status, SandboxStatus::Active);
    let restored_previous = harness
        .store
        .get_agent(&previous.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored_previous.status, AgentStatus::Ready);
    assert_eq!(
        restored_previous.conversation.conversation_id,
        previous.conversation.conversation_id
    );
    let waiting = harness
        .store
        .get_agent(&new_agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(waiting.status, AgentStatus::Starting);
    assert_eq!(waiting.work.step, AgentWorkStep::AddWorktree);
    let start_at = revive_at + Duration::from_secs(2);
    for _ in 0..3 {
        agent_worker.run_one(&cancelled.id, start_at).await.unwrap();
    }
    assert_eq!(
        harness
            .store
            .get_agent(&cancelled.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Stopped
    );
    for _ in 0..6 {
        agent_worker.run_one(&new_agent.id, start_at).await.unwrap();
    }
    let started = harness
        .store
        .get_agent(&new_agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.status, AgentStatus::Working);
    assert_eq!(started.work.step, AgentWorkStep::None);
    let message = started.conversation.first_message.as_ref().unwrap();
    assert_eq!(message.state, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(message.attempt, 1);
    assert_eq!(
        harness
            .store
            .agent_log(&new_agent.id, 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    assert_eq!(
        harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Install(_)))
            .count(),
        1
    );
}

pub(super) fn workspace_for(agent: &Agent, now: SystemTime) -> Workspace {
    let content = WorkspaceProfileContent {
        location: WorkspaceLocation::Directory {
            name: "project-1".into(),
        },
        name: "project-1".into(),
        setup_commands: Vec::new(),
        agent_profiles: Vec::new(),
    };
    Workspace {
        id: agent.workspace_id.clone(),
        sandbox_id: agent.sandbox_id.clone(),
        owner: agent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId::generate(),
            revision: 1,
            digest: layer2_core::profiles::content_digest(&content).unwrap(),
            content,
        },
        name: "project-1".into(),
        path: "/home/cma/project-1".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, rfc3339_at(now)),
        next_worktree_number: 2,
        created_at: rfc3339_at(now),
        ready_at: Some(rfc3339_at(now)),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

#[tokio::test]
async fn paused_workspace_create_restores_before_prepare_and_setup() {
    use crate::sandbox::workspace_worker::{
        WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
    };

    let harness = Harness::new(vec!["install-once".into()], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, pause_at).await;
    }
    let revive_at = pause_at + Duration::from_secs(1);
    let template = lifecycle_queue_tests::ready_agent(&active, now);
    let mut workspace = workspace_for(&template, revive_at);
    workspace.status = WorkspaceStatus::Preparing;
    workspace.resolved_commit = None;
    workspace.ready_at = None;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, rfc3339_at(revive_at));
    workspace.profile.content.setup_commands = vec!["mkdir -p local".into()];
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    harness
        .store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: workspace.clone(),
            kind: WorkspaceAdmissionKind::Manual,
            event_at: rfc3339_at(revive_at),
        })
        .await
        .unwrap();
    let mut config = WorkspaceWorkerConfig::new("new-workspace-worker", "test");
    config.retry_delay = Duration::from_secs(1);
    let worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
        },
        config,
    );
    worker.run_one(&workspace.id, revive_at).await.unwrap();
    assert!(!harness
        .backend
        .requests()
        .await
        .iter()
        .any(|(_, request)| matches!(request, RequestFrame::PrepareWorkspace(_))));
    for _ in 0..8 {
        restarted_worker(&harness, revive_at)
            .await
            .run_one(&active.id, revive_at)
            .await
            .unwrap();
    }
    assert_eq!(
        harness.stored(&active.id).await.status,
        SandboxStatus::Active
    );
    let prepare_at = revive_at + Duration::from_secs(2);
    for _ in 0..2 {
        worker.run_one(&workspace.id, prepare_at).await.unwrap();
    }
    assert_eq!(
        harness
            .store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkspaceStatus::Ready
    );
    let requests = harness.backend.requests().await;
    let restore = requests
        .iter()
        .position(|(_, request)| matches!(request, RequestFrame::Restore(_)))
        .unwrap();
    let prepare = requests
        .iter()
        .position(|(_, request)| matches!(request, RequestFrame::PrepareWorkspace(_)))
        .unwrap();
    assert!(restore < prepare);
    assert_eq!(
        requests
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Install(_)))
            .count(),
        1
    );
}
