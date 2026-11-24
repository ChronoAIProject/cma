use super::*;

use layer2_core::sandbox::store::{AgentLogStore, AgentStore, WorktreeStore};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, FirstMessageRecord, Worktree, WorktreeStatus,
};
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, WorkspaceRequest, WorktreeRequest};

use crate::sandbox::reconciliation::SandboxReconciler;
use crate::sandbox::runtime_mapping::runtime_options;

#[tokio::test]
async fn restart_during_every_pause_and_revive_boundary_preserves_two_conversations_and_worktrees()
{
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let mut active = harness.drive_active(&created.id, now).await;
    let revision = active.revision;
    active.runtime.as_mut().unwrap().runtime_version = image();
    active.revision += 1;
    harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    let first = prepare_agent(&harness, &active, now, 1).await;
    let second = prepare_agent(&harness, &active, now, 2).await;
    let original = harness
        .backend
        .snapshot(&RuntimeHandle(
            active.runtime.as_ref().unwrap().runtime_id.clone(),
        ))
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
        restarted_worker(&harness, pause_at)
            .await
            .run_one(&active.id, pause_at)
            .await
            .unwrap();
    }
    let paused = harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    assert_eq!(
        paused
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .unwrap()
            .conversations
            .len(),
        2
    );
    assert_eq!(
        harness
            .store
            .get_agent(&first.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    assert_eq!(
        harness
            .store
            .get_agent(&second.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );

    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..8 {
        restarted_worker(&harness, revive_at)
            .await
            .run_one(&active.id, revive_at)
            .await
            .unwrap();
    }
    let revived = harness.stored(&active.id).await;
    assert_eq!(revived.status, SandboxStatus::Active);
    let restored = harness
        .backend
        .snapshot(&RuntimeHandle(revived.runtime.unwrap().runtime_id))
        .await
        .unwrap();
    assert_eq!(restored.worktrees, original.worktrees);
    assert_eq!(restored.homes, original.homes);
    assert_eq!(restored.agents.len(), 2);
    for before in [first, second] {
        let after = harness.store.get_agent(&before.id).await.unwrap().unwrap();
        assert_eq!(after.status, AgentStatus::Ready);
        assert_eq!(after.worktree_id, before.worktree_id);
        assert_eq!(
            after.conversation.conversation_id,
            before.conversation.conversation_id
        );
        assert!(after.conversation.runtime_generation > before.conversation.runtime_generation);
    }
}

#[tokio::test]
async fn checkpoint_conversations_stopped_after_capture_do_not_prevent_reopening_current_live_agents(
) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let live = prepare_agent(&harness, &active, now, 1).await;
    let stopped = prepare_agent(&harness, &active, now, 2).await;
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
    let mut stopped = harness.store.get_agent(&stopped.id).await.unwrap().unwrap();
    let revision = stopped.revision;
    stopped
        .transition(
            AgentStatus::Stopped,
            rfc3339_at(pause_at),
            Some(layer2_core::sandbox::AgentStopReason::Owner),
        )
        .unwrap();
    harness.store.save_agent(revision, stopped).await.unwrap();
    let mut live = harness.store.get_agent(&live.id).await.unwrap().unwrap();
    let revision = live.revision;
    live.conversation.first_message = Some(
        FirstMessageRecord::new("unattempted-first", "First input", rfc3339_at(pause_at)).unwrap(),
    );
    live.revision += 1;
    harness
        .store
        .save_agent(revision, live.clone())
        .await
        .unwrap();
    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..6 {
        harness.run(&active.id, revive_at).await;
    }
    let reopened = harness.store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(reopened.status, AgentStatus::Ready);
    assert_eq!(reopened.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(
        reopened.conversation.first_message,
        live.conversation.first_message
    );
    assert_eq!(
        reopened.conversation.conversation_id,
        live.conversation.conversation_id
    );
}

fn image() -> String {
    CodexSandboxSpec::default().deployment.image.uri
}

mod connection_recovery_tests;
mod creation_revival;

pub(super) fn workspace_for(agent: &Agent, now: SystemTime) -> layer2_core::sandbox::Workspace {
    creation_revival::workspace_for(agent, now)
}
mod service_access;

pub(super) async fn restarted_worker(harness: &Harness, now: SystemTime) -> Arc<SandboxWorker> {
    let adapter: Arc<dyn SandboxAdapter> = Arc::new(CodexAdapter::with_backend(
        harness.backend.clone(),
        CodexSandboxSpec::default(),
    ));
    SandboxReconciler::new(
        harness.store.clone(),
        adapter.clone(),
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(now)
    .await
    .unwrap();
    let registry = Arc::new(LiveAgentRegistry::default());
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            credentials: harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: harness.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: harness.actions.clone(),
            }),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), registry),
        },
        SandboxWorkerConfig::new("restarted-worker", "test", image(), harness.policy.clone()),
    )
}

pub(super) async fn prepare_agent(
    harness: &Harness,
    sandbox: &Sandbox,
    now: SystemTime,
    number: u64,
) -> Agent {
    prepare_agent_with_conversation(harness, sandbox, now, number, None, None).await
}

pub(super) async fn prepare_agent_with_conversation(
    harness: &Harness,
    sandbox: &Sandbox,
    now: SystemTime,
    number: u64,
    conversation: Option<String>,
    settings: Option<layer2_core::sandbox::AgentSettings>,
) -> Agent {
    let mut agent = lifecycle_queue_tests::ready_agent(sandbox, now);
    agent.conversation.settings = settings;
    agent.creation_key = format!("restore-agent-{number}");
    agent.work.next_fence = 3;
    let runtime = RuntimeHandle(sandbox.runtime.as_ref().unwrap().runtime_id.clone());
    let name = format!("project-{number}");
    harness
        .adapter
        .prepare_workspace(
            &runtime,
            &WorkspaceRequest::new(
                format!("workspace:{}:prepare:v1", agent.workspace_id),
                agent.workspace_id.to_string(),
                &name,
                layer3_adapter::WorkspaceSource::Init,
            ),
        )
        .await
        .unwrap();
    let receipt = harness
        .adapter
        .add_worktree(
            &runtime,
            1,
            &WorktreeRequest::new(
                format!("worktree:{name}:wt-1:add:v1"),
                &name,
                "wt-1",
                "a".repeat(40),
            ),
        )
        .await
        .unwrap();
    let worktree = Worktree {
        id: agent.worktree_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        name: "wt-1".into(),
        path: receipt.path,
        branch: receipt.branch,
        base_commit: receipt.base_commit,
        status: WorktreeStatus::Ready,
        agent_id: Some(agent.id.clone()),
        revision: 1,
    };
    let options = runtime_options(&agent, sandbox, &worktree);
    harness
        .adapter
        .materialize_agent_home(
            &runtime,
            1,
            &AgentHomeRequest::new(
                format!("agent:{}:home:create:v1", agent.id),
                options.clone(),
                "",
                Vec::new(),
            ),
        )
        .await
        .unwrap();
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let channel = harness
        .adapter
        .open_agent(
            &runtime,
            2,
            &AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, conversation),
            updates,
        )
        .await
        .unwrap();
    agent.conversation.conversation_id = channel.conversation_id;
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 2;
    harness.store.create_worktree(worktree).await.unwrap();
    harness.store.create_agent(agent.clone()).await.unwrap();
    for kind in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    ] {
        harness
            .store
            .append_agent_log(&agent.id, rfc3339_at(now), kind)
            .await
            .unwrap();
    }
    harness.store.get_agent(&agent.id).await.unwrap().unwrap()
}
