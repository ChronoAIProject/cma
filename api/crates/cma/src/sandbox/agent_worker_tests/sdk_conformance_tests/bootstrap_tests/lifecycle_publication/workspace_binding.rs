use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn workspace_wait_requeue_rebuilds_after_a_concurrent_workspace_revision() {
    let fixture = Fixture::new().await;
    let h = &fixture.bootstrap.harness;
    let waiting = fixture.bootstrap.admit(Some("queued prompt")).await;
    let (claimed, now) = fixture.claim(&waiting.id).await;
    let requests_before = h.backend.requests().await.len();
    let workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    fixture.trace.bodies.store(0, Ordering::SeqCst);
    *fixture.trace.workspace_advance.lock().unwrap() = Some(WorkspaceAdvance {
        id: workspace.id.clone(),
        transaction: 1,
    });

    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await;
    let updated = h.store.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_eq!(updated.revision, workspace.revision + 1);
    result.unwrap();
    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 2);
    let deferred = h.stored(&claimed.id).await;
    assert_eq!(deferred.work.step, AgentWorkStep::WaitForWorkspace);
    assert!(deferred.work.lease.is_none());
    assert_eq!(deferred.work.ready_at, deferred.updated_at);
    assert_eq!(
        deferred.conversation.first_message,
        claimed.conversation.first_message
    );
    assert_eq!(h.backend.requests().await.len(), requests_before);
    fixture.bootstrap.ready().await;
    let (next_claim, now) = fixture.claim(&waiting.id).await;
    h.worker
        .execute_claimed_for_test(next_claim, now)
        .await
        .unwrap();
    let ready = h.store.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_bound(&fixture, &claimed, &ready, requests_before).await;
    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn wait_binding_rebuilds_after_a_concurrent_workspace_revision() {
    let fixture = Fixture::new().await;
    let h = &fixture.bootstrap.harness;
    let waiting = fixture.bootstrap.admit(Some("queued prompt")).await;
    fixture.bootstrap.ready().await;
    let (claimed, now) = fixture.claim(&waiting.id).await;
    let requests_before = h.backend.requests().await.len();
    let workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    fixture.trace.bodies.store(0, Ordering::SeqCst);
    *fixture.trace.workspace_advance.lock().unwrap() = Some(WorkspaceAdvance {
        id: workspace.id.clone(),
        transaction: 1,
    });

    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await;
    let current = h.store.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_eq!(current.revision, workspace.revision + 1);
    assert_eq!(
        current.next_worktree_number,
        workspace.next_worktree_number + 1
    );
    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 2);
    result.unwrap();
    assert_bound(&fixture, &claimed, &current, requests_before).await;
    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn exhausted_wait_binding_releases_the_lease_and_is_discovered_again() {
    let fixture = Fixture::new().await;
    let h = &fixture.bootstrap.harness;
    let waiting = fixture.bootstrap.admit(Some("queued prompt")).await;
    fixture.bootstrap.ready().await;
    let (claimed, now) = fixture.claim(&waiting.id).await;
    let requests_before = h.backend.requests().await.len();
    let original_tree = h
        .store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let original_workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    fixture.transient(doc! {"times": 5_i32}.into()).await;
    *fixture.trace.workspace_advance.lock().unwrap() = Some(WorkspaceAdvance {
        id: original_workspace.id.clone(),
        transaction: 6,
    });

    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await;
    fixture.disable().await;
    result.unwrap();
    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 7);
    assert_eq!(fixture.trace.commits.load(Ordering::SeqCst), 1);
    let queued = h.stored(&claimed.id).await;
    assert_eq!(queued.work.step, AgentWorkStep::WaitForWorkspace);
    assert_eq!(queued.work.attempts, claimed.work.attempts + 1);
    assert!(queued.work.lease.is_none());
    assert_eq!(
        queued.conversation.first_message,
        claimed.conversation.first_message
    );
    assert_eq!(queued.conversation.last_seq, claimed.conversation.last_seq);
    assert_eq!(
        h.store
            .get_worktree(&claimed.worktree_id)
            .await
            .unwrap()
            .unwrap(),
        original_tree
    );
    assert_eq!(h.backend.requests().await.len(), requests_before);
    let published_at = layer2_core::seconds_from_rfc3339(&queued.updated_at).unwrap();
    let ready_at = layer2_core::seconds_from_rfc3339(&queued.work.ready_at).unwrap();
    assert_eq!(ready_at, published_at + 1);
    assert!(queued.work.ready_at < claimed.work.lease.as_ref().unwrap().expires_at);
    let due = SystemTime::UNIX_EPOCH + Duration::from_secs(ready_at);
    let store = Arc::new(
        MongoSandboxStore::open(&fixture.admin, &fixture.database)
            .await
            .unwrap(),
    );
    let registry = Arc::new(LiveAgentRegistry::default());
    let successor = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry),
        },
        AgentWorkerConfig::new("bootstrap-successor", DEPLOYMENT_ID),
    );
    assert!(!successor
        .run_one(&claimed.id, due - Duration::from_secs(1))
        .await
        .unwrap());
    assert_eq!(h.stored(&claimed.id).await, queued);
    assert_eq!(
        successor.due_work(due, 10).await.unwrap(),
        vec![claimed.id.clone()]
    );
    assert!(successor.run_one(&claimed.id, due).await.unwrap());
    let workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workspace.revision, original_workspace.revision + 1);
    assert_bound(&fixture, &claimed, &workspace, requests_before).await;
    let advanced = h.stored(&claimed.id).await;
    assert_eq!(advanced.work.next_fence, claimed.work.next_fence + 1);
    let tree = h
        .store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tree.revision, original_tree.revision + 1);
    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn wait_binding_preserves_claim_when_workspace_cleanup_owns_the_parent() {
    let fixture = Fixture::new().await;
    let h = &fixture.bootstrap.harness;
    let waiting = fixture.bootstrap.admit(None).await;
    fixture.bootstrap.ready().await;
    let (claimed, now) = fixture.claim(&waiting.id).await;
    let requests_before = h.backend.requests().await.len();
    // Simulate cleanup admitted after the Agent claim, with a newer parent
    // revision. The new revision cannot authorize publishing through custody.
    let update = fixture
        .admin
        .database(&fixture.database)
        .collection::<Document>("workspaces")
        .update_one(
            doc! {"_id": claimed.workspace_id.as_str()},
            doc! {
                "$set": {"owned_workspace_cleanup_admission_v1": {"pending": true}},
                "$inc": {"revision": 1_i64, "record.revision": 1_i64},
            },
        )
        .await
        .unwrap();
    assert_eq!(update.matched_count, 1);
    fixture.trace.bodies.store(0, Ordering::SeqCst);
    assert_eq!(
        h.worker
            .execute_claimed_for_test(claimed.clone(), now)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 1);
    assert_eq!(h.stored(&claimed.id).await, claimed);
    assert_eq!(h.backend.requests().await.len(), requests_before);
    fixture.cleanup().await;
}

async fn assert_bound(
    fixture: &Fixture,
    claimed: &Agent,
    workspace: &Workspace,
    requests_before: usize,
) {
    let h = &fixture.bootstrap.harness;
    let agent = h.stored(&claimed.id).await;
    assert_eq!(agent.work.step, AgentWorkStep::AddWorktree);
    assert!(agent.work.lease.is_none());
    assert_eq!(
        agent.conversation.first_message,
        claimed.conversation.first_message
    );
    let tree = h
        .store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tree.status, WorktreeStatus::Creating);
    assert_eq!(Some(&tree.base_commit), workspace.resolved_commit.as_ref());
    let logs = h.store.agent_log(&claimed.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(
                entry.kind,
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::WaitForWorkspace
                }
            ))
            .count(),
        1
    );
    assert_eq!(h.backend.requests().await.len(), requests_before);
}
