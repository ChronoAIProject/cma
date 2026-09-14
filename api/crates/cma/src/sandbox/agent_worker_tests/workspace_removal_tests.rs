use super::*;
use crate::sandbox::workspace_worker::{
    WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
};
use layer2_core::sandbox::store::{
    AgentStopRequest, SandboxStoreError, WorkspaceAdmissionStore as _,
};
use layer3_adapter::wire::RequestFrame;

fn workspace_worker(harness: &Harness) -> Arc<WorkspaceWorker> {
    WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
        },
        WorkspaceWorkerConfig::new("workspace-removal", DEPLOYMENT_ID),
    )
}

#[tokio::test]
async fn workspace_removal_survives_worker_recreation_and_ambiguous_child_close() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let first = harness.admit("first", Vec::new()).await;
    let second = harness.admit("second", Vec::new()).await;
    let ready = harness.drive_ready(&first.id).await;
    harness.drive_ready(&second.id).await;
    harness
        .store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Operator,
            requested_at: rfc3339_at(harness.now),
        })
        .await
        .unwrap();
    let removing = harness
        .store
        .begin_workspace_removal(
            &harness.workspace.id,
            &harness.workspace.owner,
            rfc3339_at(harness.now),
        )
        .await
        .unwrap();
    assert_eq!(removing.status, WorkspaceStatus::Removing);
    assert_eq!(harness.stored(&first.id).await.status, AgentStatus::Ready);
    assert_eq!(
        harness
            .store
            .begin_workspace_removal(
                &harness.workspace.id,
                &harness.workspace.owner,
                rfc3339_at(harness.now)
            )
            .await
            .unwrap(),
        removing
    );
    harness
        .backend
        .lose_next_response(WireOperation::CloseAgent);
    let mut complete = false;
    for tick in 0..90 {
        let now = harness.now + Duration::from_secs(tick);
        // Each pass is a new worker: only persisted Workspace and Agent work can advance it.
        workspace_worker(&harness)
            .run_one(&harness.workspace.id, now)
            .await
            .unwrap();
        if tick == 0 {
            assert_eq!(
                harness.stored(&first.id).await.requested_stop_reason,
                Some(AgentStopReason::Operator)
            );
            assert_eq!(
                harness.stored(&second.id).await.requested_stop_reason,
                Some(AgentStopReason::Owner)
            );
        }
        let workspace = harness
            .store
            .get_workspace(&harness.workspace.id)
            .await
            .unwrap()
            .unwrap();
        if workspace.status == WorkspaceStatus::Removed {
            complete = true;
            break;
        }
        assert!(!harness
            .backend
            .requests()
            .await
            .iter()
            .any(|(_, request)| matches!(request, RequestFrame::RemoveWorkspace(_))));
        for id in [&first.id, &second.id] {
            harness.worker.run_one(id, now).await.unwrap();
        }
    }
    assert!(complete, "durable cascade did not finish");
    for id in [&first.id, &second.id] {
        let agent = harness.stored(id).await;
        assert_eq!(agent.status, AgentStatus::Deleted);
        assert_eq!(agent.work.step, AgentWorkStep::None);
        assert_eq!(
            harness
                .store
                .get_worktree(&agent.worktree_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorktreeStatus::Removed
        );
    }
    assert_eq!(
        harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::RemoveWorkspace(_)))
            .count(),
        1
    );
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn removing_workspace_fences_new_agent_admission_without_requiring_an_owner_stop() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("starting", Vec::new()).await;
    harness
        .store
        .begin_workspace_removal(
            &harness.workspace.id,
            &harness.workspace.owner,
            rfc3339_at(harness.now),
        )
        .await
        .unwrap();
    let result = harness
        .store
        .admit_agent(AgentAdmission {
            owner: harness.workspace.owner.clone(),
            workspace_id: harness.workspace.id.clone(),
            agent_id: AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile: agent.profile.clone(),
            capabilities: agent.capabilities.clone(),
            title: None,
            first_message: None,
            origin: AgentOrigin::Manual,
            creation_key: "after-removal".into(),
            event_at: rfc3339_at(harness.now),
        })
        .await;
    assert!(matches!(result, Err(SandboxStoreError::WorkspaceNotReady)));
    workspace_worker(&harness)
        .run_one(&harness.workspace.id, harness.now)
        .await
        .unwrap();
    assert_eq!(
        harness.stored(&agent.id).await.requested_stop_reason,
        Some(AgentStopReason::Owner)
    );
    for tick in 1..20 {
        let now = harness.now + Duration::from_secs(tick);
        harness.worker.run_one(&agent.id, now).await.unwrap();
        workspace_worker(&harness)
            .run_one(&harness.workspace.id, now)
            .await
            .unwrap();
    }
    assert_eq!(harness.stored(&agent.id).await.status, AgentStatus::Deleted);
    assert_eq!(
        harness
            .store
            .get_workspace(&harness.workspace.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkspaceStatus::Removed
    );
}

#[tokio::test]
async fn workspace_removal_rejects_foreign_owner_and_does_not_stop_agents_elsewhere() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("retained", Vec::new()).await;
    harness.drive_ready(&agent.id).await;
    assert_eq!(
        harness
            .store
            .begin_workspace_removal(
                &harness.workspace.id,
                &SubjectId::new("other-owner"),
                rfc3339_at(harness.now)
            )
            .await,
        Err(SandboxStoreError::NotFound)
    );
    let mut empty = harness.workspace.clone();
    empty.id = WorkspaceId::generate();
    empty.name = "empty".into();
    empty.path = "/home/cma/empty".into();
    empty.profile.content.name = "empty".into();
    empty.profile.content.location = WorkspaceLocation::Directory {
        name: "empty".into(),
    };
    let receipt = harness
        .adapter
        .prepare_workspace(
            &harness.runtime,
            &WorkspaceRequest::new(
                "workspace:empty:prepare:v1",
                empty.id.to_string(),
                "empty",
                AdapterWorkspaceSource::Init,
            ),
        )
        .await
        .unwrap();
    empty.resolved_commit = Some(receipt.resolved_commit);
    harness.store.create_workspace(empty.clone()).await.unwrap();
    harness
        .store
        .begin_workspace_removal(&empty.id, &empty.owner, rfc3339_at(harness.now))
        .await
        .unwrap();
    assert!(workspace_worker(&harness)
        .run_one(&empty.id, harness.now)
        .await
        .unwrap());
    assert_eq!(
        harness
            .store
            .get_workspace(&empty.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkspaceStatus::Removed
    );
    assert_eq!(harness.stored(&agent.id).await.status, AgentStatus::Ready);
    assert_eq!(harness.stored(&agent.id).await.requested_stop_reason, None);
}
