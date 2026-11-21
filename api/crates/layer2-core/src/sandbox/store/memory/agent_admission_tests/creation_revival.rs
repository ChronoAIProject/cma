use super::*;
use crate::sandbox::lifecycle_test_support::checkpoint;
use crate::sandbox::store::{
    SandboxEventStore, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionResult,
    WorkspaceAdmissionStore,
};
use crate::sandbox::{
    AgentCleanupDebt, AgentWorkReceipt, AgentWorkStep, SandboxEventKind, SandboxLifecycleOperation,
    SandboxReviveCause,
};

fn pause(store: &InMemorySandboxStore) -> Sandbox {
    let mut state = store.lock();
    let parent = state.sandboxes.get_mut(&sandbox(4).id).unwrap();
    parent.status = SandboxStatus::Paused;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    parent.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
    parent.validate().unwrap();
    parent.clone()
}

fn workspace_request(name: &str) -> WorkspaceAdmission {
    let mut value = workspace();
    value.id = WorkspaceId::generate();
    value.name = name.into();
    value.path = format!("/home/cma/{name}");
    value.status = WorkspaceStatus::Preparing;
    value.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    value.resolved_commit = None;
    value.ready_at = None;
    WorkspaceAdmission {
        binding: None,
        workspace: value,
        kind: WorkspaceAdmissionKind::Manual,
        event_at: NOW.into(),
    }
}

#[tokio::test]
async fn paused_agent_admission_revives_once_and_replay_keeps_the_reservation() {
    let store = ready_store(4).await;
    let parent = pause(&store);
    let request = admission("paused-create");
    let (left, right) = tokio::join!(
        store.admit_agent(request.clone()),
        store.admit_agent(request.clone())
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, AgentAdmissionResult::Created(_)))
            .count(),
        1
    );
    assert_eq!(results[0].creation(), results[1].creation());
    let reviving = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(reviving.status, SandboxStatus::Reviving);
    assert_eq!(
        reviving.lifecycle.next_authority,
        parent.lifecycle.next_authority + 1
    );
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::AgentCreation,
            ..
        })
    ));
    assert!(!reviving.restores_agent(&results[0].creation().agent));
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 20).await.unwrap().len(),
        3
    );

    let mut claimed = reviving.clone();
    claimed
        .work
        .claim("restore-worker".into(), NOW, "2026-09-05T00:15:00Z".into())
        .unwrap();
    store
        .lock()
        .sandboxes
        .insert(parent.id.clone(), claimed.clone());
    store
        .admit_agent(admission("during-revival"))
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(claimed));
}

#[tokio::test]
async fn paused_agent_rejections_do_not_schedule_revival_or_allocate_worktrees() {
    let store = ready_store(1).await;
    store.admit_agent(admission("existing")).await.unwrap();
    let paused = pause(&store);
    assert!(matches!(
        store.admit_agent(admission("over-limit")).await,
        Err(SandboxStoreError::AgentLimitReached { .. })
    ));
    let mut foreign = admission("foreign");
    foreign.owner = SubjectId::new("other-owner");
    assert_eq!(
        store.admit_agent(foreign).await.unwrap_err(),
        SandboxStoreError::WorkspaceNotReady
    );
    assert_eq!(store.get_sandbox(&paused.id).await.unwrap(), Some(paused));
    assert_eq!(store.lock().worktrees.len(), 1);
}

#[tokio::test]
async fn paused_workspace_admission_is_atomic_and_conflicts_leave_it_paused() {
    let store = ready_store(4).await;
    let paused = pause(&store);
    assert_eq!(
        store
            .admit_workspace(workspace_request("project"))
            .await
            .unwrap_err(),
        SandboxStoreError::WorkspaceNameConflict
    );
    assert!(matches!(
        store.admit_workspace(workspace_request("second")).await,
        Err(SandboxStoreError::WorkspaceLimitReached { .. })
    ));
    assert_eq!(
        store.get_sandbox(&paused.id).await.unwrap(),
        Some(paused.clone())
    );
    store.lock().workspaces.clear();
    let request = workspace_request("new-project");
    let admitted = store.admit_workspace(request).await.unwrap();
    assert!(matches!(admitted, WorkspaceAdmissionResult::Created(_)));
    let reviving = store.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::WorkspaceCreation,
            ..
        })
    ));
    let events = store.sandbox_events(&paused.id, 0, 20).await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Paused,
            to: SandboxStatus::Reviving
        }
    ));
}

#[tokio::test]
async fn restore_exclusion_requires_no_checkpoint_membership_or_runtime_effect_history() {
    let store = ready_store(4).await;
    let paused = pause(&store);
    let creation = created(store.admit_agent(admission("unstarted")).await.unwrap());
    let reviving = store.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(!reviving.restores_agent(&creation.agent));
    let mut changed = creation.agent.clone();
    changed.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::AddWorktree,
        operation_id: "add".into(),
        request_sha256: "a".repeat(64),
    });
    assert!(reviving.restores_agent(&changed));
    changed.work.cleanup_debt.clear();
    changed.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::AddWorktree,
        operation_id: "add".into(),
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    });
    assert!(reviving.restores_agent(&changed));
    let mut checkpoint_member = creation.agent;
    checkpoint_member
        .request_stop(crate::sandbox::AgentStopReason::Owner, NOW.into())
        .unwrap();
    assert!(!reviving.restores_agent(&checkpoint_member));
    checkpoint_member.ready_at = Some(NOW.into());
    assert!(reviving.restores_agent(&checkpoint_member));
    checkpoint_member.ready_at = None;
    checkpoint_member.work.resolved_skills = Some(Vec::new());
    assert!(reviving.restores_agent(&checkpoint_member));
    checkpoint_member.work.resolved_skills = None;
    checkpoint_member.id = AgentId("agt_a".into());
    assert!(reviving.restores_agent(&checkpoint_member));
}
