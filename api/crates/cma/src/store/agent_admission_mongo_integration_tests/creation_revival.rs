use super::*;
use layer2_core::sandbox::store::{
    SandboxEventStore, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionStore,
};
use layer2_core::sandbox::{
    SandboxCheckpointRef, SandboxEventKind, SandboxLifecycleOperation, SandboxReviveCause,
};
use layer3_adapter::{CheckpointReceipt, WriterState};

async fn pause(store: &MongoSandboxStore, mut sandbox: Sandbox) -> Sandbox {
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Paused;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    sandbox.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/paused-create.tar.zst".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &[],
            NOW,
        )
        .unwrap(),
    );
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox.clone()).await.unwrap();
    sandbox
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn paused_agent_admission_replays_one_revival_after_store_reopen() {
    let (client, database, store, sandbox, workspace, request) = fixture("paused_replay").await;
    let paused = pause(&store, sandbox).await;
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
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let reviving = reopened.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::AgentCreation,
            ..
        })
    ));
    assert_eq!(
        reopened.admit_agent(request).await.unwrap().creation(),
        results[0].creation()
    );
    assert_eq!(
        reopened.get_sandbox(&paused.id).await.unwrap(),
        Some(reviving.clone())
    );
    reopened
        .admit_agent(admission(&workspace, "during_revival"))
        .await
        .unwrap();
    assert_eq!(
        reopened.get_sandbox(&paused.id).await.unwrap(),
        Some(reviving)
    );
    assert_eq!(
        reopened
            .sandbox_events(&paused.id, 0, 20)
            .await
            .unwrap()
            .iter()
            .filter(|event| matches!(
                event.kind,
                SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Paused,
                    to: SandboxStatus::Reviving
                }
            ))
            .count(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn paused_agent_quota_and_owner_refusals_do_not_revive() {
    let (client, database, store, mut sandbox, _, request) = fixture("paused_refusal").await;
    let sandbox_id = sandbox.id.clone();
    let revision = sandbox.revision;
    sandbox.limits.max_agents_per_workspace = 1;
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox).await.unwrap();
    store.admit_agent(request.clone()).await.unwrap();
    let parent = store.get_sandbox(&sandbox_id).await.unwrap().unwrap();
    let paused = pause(&store, parent).await;
    let mut refused = request;
    refused.creation_key = "over-limit".into();
    refused.agent_id = AgentId::generate();
    refused.worktree_id = WorktreeId::generate();
    assert!(matches!(
        store.admit_agent(refused.clone()).await,
        Err(SandboxStoreError::AgentLimitReached { .. })
    ));
    refused.owner = SubjectId::new("different-owner");
    assert_eq!(
        store.admit_agent(refused).await.unwrap_err(),
        SandboxStoreError::WorkspaceNotReady
    );
    assert_eq!(store.get_sandbox(&paused.id).await.unwrap(), Some(paused));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn paused_workspace_name_conflict_does_not_revive_and_create_is_durable() {
    let (client, database, store, sandbox, workspace, _) = fixture("paused_workspace").await;
    let paused = pause(&store, sandbox).await;
    let mut candidate = workspace.clone();
    candidate.id = WorkspaceId::generate();
    candidate.status = WorkspaceStatus::Preparing;
    candidate.resolved_commit = None;
    candidate.ready_at = None;
    candidate.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let mut request = WorkspaceAdmission {
        binding: None,
        workspace: candidate,
        kind: WorkspaceAdmissionKind::Manual,
        event_at: NOW.into(),
    };
    assert_eq!(
        store.admit_workspace(request.clone()).await.unwrap_err(),
        SandboxStoreError::WorkspaceNameConflict
    );
    assert_eq!(
        store.get_sandbox(&paused.id).await.unwrap(),
        Some(paused.clone())
    );
    request.workspace.name = "new-project".into();
    request.workspace.path = "/home/cma/new-project".into();
    store.admit_workspace(request.clone()).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.get_workspace(&request.workspace.id).await.unwrap(),
        Some(request.workspace)
    );
    let reviving = reopened.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::WorkspaceCreation,
            ..
        })
    ));
    assert_eq!(
        reopened
            .sandbox_events(&paused.id, 0, 20)
            .await
            .unwrap()
            .len(),
        2
    );
    client.database(&database).drop().await.unwrap();
}
