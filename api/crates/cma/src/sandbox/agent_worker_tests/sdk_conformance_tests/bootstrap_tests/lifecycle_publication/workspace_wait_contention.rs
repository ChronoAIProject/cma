use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn workspace_wait_exhausted_abort_releases_the_claim_with_unchanged_parents() {
    let fixture = Fixture::new().await;
    let h = &fixture.bootstrap.harness;
    let waiting = fixture.bootstrap.admit(Some("queued prompt")).await;
    let (claimed, now) = fixture
        .claim_for(&waiting.id, Duration::from_secs(18 * 60))
        .await;
    let lease = claimed.work.lease.as_ref().unwrap();
    let requests_before = h.backend.requests().await.len();
    let tree_before = h
        .store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();

    // Each transaction aborts before publication. No parent revision advances.
    fixture.transient(doc! {"times": 5_i32}.into()).await;
    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await;
    fixture.disable().await;
    let deferred = h.stored(&claimed.id).await;
    let released = deferred.work.lease.is_none();
    let transaction_bodies = fixture.trace.bodies.load(Ordering::SeqCst);
    if result.is_err() || !released {
        fixture.cleanup().await;
        panic!("workspace wait did not release its claim after {transaction_bodies} transactions: {result:?}, released={released}");
    }
    assert!(transaction_bodies >= 6);
    assert_eq!(deferred.work.step, AgentWorkStep::WaitForWorkspace);
    assert_eq!(deferred.conversation, claimed.conversation);
    assert_eq!(h.backend.requests().await.len(), requests_before);
    assert_eq!(
        h.store
            .get_worktree(&claimed.worktree_id)
            .await
            .unwrap()
            .unwrap(),
        tree_before
    );
    assert!(deferred.work.ready_at < lease.expires_at);

    fixture.bootstrap.ready().await;
    let due = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&deferred.work.ready_at).unwrap());
    assert_eq!(
        h.worker.due_work(due, 10).await.unwrap(),
        vec![claimed.id.clone()]
    );
    assert!(h.worker.run_one(&claimed.id, due).await.unwrap());
    let workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_bound(&fixture, &claimed, &workspace, requests_before).await;
    assert_eq!(
        h.stored(&claimed.id).await.work.next_fence,
        claimed.work.next_fence + 1
    );
    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn workspace_wait_claim_expires_for_rediscovery_when_requeue_also_aborts() {
    let fixture = Fixture::new().await;
    let h = &fixture.bootstrap.harness;
    let waiting = fixture.bootstrap.admit(Some("queued prompt")).await;
    let (claimed, now) = fixture
        .claim_for(&waiting.id, Duration::from_secs(18 * 60))
        .await;
    let lease = claimed.work.lease.as_ref().unwrap();
    let requests_before = h.backend.requests().await.len();

    fixture.transient(doc! {"times": 20_i32}.into()).await;
    let outcome = h
        .worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await;
    fixture.disable().await;
    assert_eq!(outcome, Err(SandboxStoreError::Conflict));
    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 20);
    assert_eq!(h.stored(&claimed.id).await, claimed);
    assert_eq!(h.backend.requests().await.len(), requests_before);

    let due = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&lease.expires_at).unwrap());
    assert!(due <= now + Duration::from_secs(30));
    fixture.bootstrap.ready().await;
    assert!(h
        .worker
        .due_work(due - Duration::from_secs(1), 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        h.worker.due_work(due, 10).await.unwrap(),
        vec![claimed.id.clone()]
    );
    assert!(h.worker.run_one(&claimed.id, due).await.unwrap());
    let workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_bound(&fixture, &claimed, &workspace, requests_before).await;
    assert_eq!(
        h.stored(&claimed.id).await.work.next_fence,
        claimed.work.next_fence + 1
    );
    fixture.cleanup().await;
}
