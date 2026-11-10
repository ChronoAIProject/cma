use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_admission_and_terminal_event_failures_roll_back_the_aggregate() {
    let (client, name) = database("workspace_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_workspace_rollback");
    store.create_sandbox(parent.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();

    let candidate = workspace(&parent, "wks_admission_rollback", "admission-rollback");
    assert_eq!(
        store
            .admit_workspace(workspace_admission(candidate.clone()))
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_workspace(&candidate.id).await.unwrap(), None);
    let parent_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent_row.get_i64("last_seq").unwrap(), 0);
    assert!(!parent_row.contains_key("workspace_admission_fence"));

    let original = workspace(&parent, "wks_terminal_rollback", "terminal-rollback");
    store.create_workspace(original.clone()).await.unwrap();
    let failure = TypedFailure {
        class: "setup_failed".into(),
        detail: "command failed".into(),
    };
    let mut failed = original.clone();
    failed.work.schedule(WorkspaceWorkStep::None, NOW.into());
    failed
        .transition(WorkspaceStatus::Failed, NOW.into(), Some(failure.clone()))
        .unwrap();
    assert_eq!(
        store
            .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                expected_revision: original.revision,
                workspace: Box::new(failed),
                event_at: NOW.into(),
                event: SandboxEventKind::WorkspaceFailed {
                    workspace_id: original.id.clone(),
                    failure,
                },
            })
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_workspace(&original.id).await.unwrap(),
        Some(original)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_claims_survive_reopen_and_fence_out_an_expired_worker() {
    let (client, name) = database("workspace_claims").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_workspace_claims");
    store.create_sandbox(parent.clone()).await.unwrap();
    let candidate = workspace(&parent, "wks_claims", "claims-project");
    store.create_workspace(candidate.clone()).await.unwrap();
    drop(store);

    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        store.due_workspace_work(NOW, 10).await.unwrap(),
        vec![candidate.id.clone()]
    );
    let first = store.claim_workspace_work(
        &candidate.id,
        "worker-a",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let second = store.claim_workspace_work(
        &candidate.id,
        "worker-b",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let (first, second) = tokio::join!(first, second);
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let mut stale = claims.into_iter().flatten().next().unwrap();
    let stale_holder = stale.work.lease.as_ref().unwrap().holder.clone();
    let stale_fence = stale.work.lease.as_ref().unwrap().fence;
    assert_eq!(stale_fence, 1);

    let takeover = store
        .claim_workspace_work(
            &candidate.id,
            "worker-c",
            "2026-09-05T00:01:00Z".into(),
            "2026-09-05T00:02:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(takeover.work.lease.as_ref().unwrap().fence, 2);
    let expected = stale.revision;
    stale
        .work
        .advance(
            &stale_holder,
            stale_fence,
            WorkspaceWorkStep::Setup,
            NOW.into(),
        )
        .unwrap();
    stale.revision += 1;
    assert_eq!(
        store.save_workspace(expected, stale).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .get_workspace(&candidate.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .lease
            .unwrap()
            .holder,
        "worker-c"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_removal_locks_the_row_and_durably_admits_live_child_cleanup() {
    let (client, name) = database("workspace_removal").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_workspace_removal");
    store.create_sandbox(parent.clone()).await.unwrap();
    let candidate = workspace(&parent, "wks_atomic", "removal-project");
    store.create_workspace(candidate.clone()).await.unwrap();
    let (_, mut agent) = worktree_and_agent(&parent.id);
    agent.workspace_id = candidate.id.clone();
    store.create_agent(agent.clone()).await.unwrap();

    let removing = store
        .begin_workspace_removal(&candidate.id, &candidate.owner, NOW.into())
        .await
        .unwrap();
    assert_eq!(removing.status, WorkspaceStatus::Removing);
    assert_eq!(removing.work.step, WorkspaceWorkStep::Remove);
    assert_eq!(removing.revision, 2);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    let raw = store
        .workspaces
        .find_one(doc! { "_id": candidate.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i64("removal_lock_fence").unwrap(), 1);
    assert_eq!(
        store
            .begin_workspace_removal(&candidate.id, &candidate.owner, NOW.into())
            .await
            .unwrap(),
        removing
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claims_are_exclusive_and_an_expired_lease_gets_a_higher_fence() {
    let (client, name) = database("claims").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = provisioning_sandbox("sbx_claims");
    store.create_sandbox(sandbox.clone()).await.unwrap();

    let first = store.claim_sandbox_work(
        &sandbox.id,
        "worker-a",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let second = store.claim_sandbox_work(
        &sandbox.id,
        "worker-b",
        NOW.into(),
        "2026-09-05T00:01:00Z".into(),
    );
    let (first, second) = tokio::join!(first, second);
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let claimed = claims.into_iter().flatten().next().unwrap();
    assert_eq!(claimed.work.lease.as_ref().unwrap().fence, 1);
    assert_eq!(claimed.revision, 2);

    let takeover = store
        .claim_sandbox_work(
            &sandbox.id,
            "worker-c",
            "2026-09-05T00:01:00Z".into(),
            "2026-09-05T00:02:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(takeover.work.lease.as_ref().unwrap().holder, "worker-c");
    assert_eq!(takeover.work.lease.as_ref().unwrap().fence, 2);
    assert_eq!(takeover.revision, 3);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn purge_transaction_removes_the_aggregate_hierarchy_events_and_checkpoints() {
    let (client, name) = database("purge").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = retired_sandbox("sbx_purge");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let sandbox_id = sandbox.id.as_str();
    store
        .workspaces
        .insert_one(doc! { "_id": "wks_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .worktrees
        .insert_one(doc! { "_id": "wkt_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .agents
        .insert_one(doc! { "_id": "agt_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .agent_log
        .insert_one(doc! { "_id": "log_purge", "agent_id": "agt_purge" })
        .await
        .unwrap();
    store
        .sandbox_events
        .insert_one(doc! { "_id": "evt_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();
    store
        .checkpoints
        .insert_one(doc! { "_id": "chk_purge", "sandbox_id": sandbox_id })
        .await
        .unwrap();

    store
        .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
        .await
        .unwrap();
    for collection in [
        &store.sandboxes,
        &store.workspaces,
        &store.worktrees,
        &store.agents,
        &store.agent_log,
        &store.sandbox_events,
        &store.checkpoints,
    ] {
        assert_eq!(
            collection.count_documents(Document::new()).await.unwrap(),
            0
        );
    }
    client.database(&name).drop().await.unwrap();
}
