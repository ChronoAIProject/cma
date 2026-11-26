use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn expired_credential_recovery_cannot_release_the_workspace_wait_claim() {
    let (fixture, waiting) = pending_initial_credential().await;
    let h = &fixture.bootstrap.harness;
    let (claimed, now) = fixture.claim_for(&waiting.id, Duration::from_secs(5)).await;
    let lease = claimed.work.lease.as_ref().unwrap();
    let expires = layer2_core::seconds_from_rfc3339(&lease.expires_at).unwrap();
    let request_count = h.backend.requests().await.len();
    let worktree = h
        .store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();

    fixture.trace.bodies.store(0, Ordering::SeqCst);
    *fixture.trace.deliver_initial_credential.lock().unwrap() = Some(claimed.sandbox_id.clone());
    // The first transaction sees the new credential. The second starts only
    // after the recovery worker has checked the claim and rebuilt the requeue.
    *fixture.trace.expire_recovery_publication.lock().unwrap() =
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(expires));

    h.worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await
        .unwrap();
    assert!(fixture
        .trace
        .expire_recovery_publication
        .lock()
        .unwrap()
        .is_none());
    assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 2);
    assert_eq!(h.stored(&claimed.id).await, claimed);
    assert_eq!(
        h.store
            .get_worktree(&claimed.worktree_id)
            .await
            .unwrap()
            .unwrap(),
        worktree,
    );
    assert_eq!(h.backend.requests().await.len(), request_count);

    fixture.bootstrap.ready().await;
    let (next, now) = fixture.claim(&waiting.id).await;
    assert!(next.work.lease.as_ref().unwrap().fence > lease.fence);
    h.worker.execute_claimed_for_test(next, now).await.unwrap();
    let workspace = h
        .store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_bound(&fixture, &claimed, &workspace, request_count).await;
    fixture.cleanup().await;
}
