use super::*;
use crate::store::{agent_capacity_fixture_fingerprint, fill_agent_capacity_fixture};
use layer2_core::sandbox::store::SandboxStoreError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn first_cleanup_preflight_retry_uses_the_legacy_delete_reserve() {
    let fixture = Fixture::new(false).await;
    let full = fill_agent_capacity_fixture(&fixture.h.store, fixture.agent.clone(), false).await;
    let through = full.conversation.last_seq;
    let prefix = agent_capacity_fixture_fingerprint(&fixture.h.store, &full.id, through).await;
    fixture.delete("original-delete").await;
    let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
    let claim = fixture.claim(&fixture.h.store, "agent-worker", now).await;
    let parent = fixture.begin_retirement(now).await;
    fixture.retire(parent, now, true).await;
    let requests = fixture.h.backend.requests().await;
    fixture
        .fail(1, doc! {"failCommands":["find"],"errorCode":2_i32})
        .await;
    let result = fixture
        .h
        .worker
        .execute_claimed_for_test(claim.clone(), now)
        .await;
    fixture.clear_fault().await;
    result.unwrap();

    let retry = fixture.stored().await;
    ledger_hold_tests::preserved(&claim, &retry);
    assert_eq!(retry.work.attempts, 1);
    assert_eq!(retry.conversation.last_seq, through + 1);
    let due = ledger_hold_tests::due(&retry);
    assert!(due > now && due < now + LEASE);
    assert!(fixture
        .h
        .worker
        .due_work(due, 100)
        .await
        .unwrap()
        .contains(&claim.id));
    fixture.converge(&fixture.h.worker, due).await;
    assert_eq!(fixture.h.backend.requests().await, requests);
    assert_eq!(
        agent_capacity_fixture_fingerprint(&fixture.h.store, &full.id, through).await,
        prefix
    );
    fixture.admin.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn cleanup_preflight_retry_cannot_commit_after_its_original_lease_expires() {
    for segmented in [false, true] {
        let fixture = Fixture::new(segmented).await;
        fixture.delete("original-delete").await;
        let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
        let claim = fixture.claim(&fixture.h.store, "agent-worker", now).await;
        let parent = fixture.begin_retirement(now).await;
        fixture.retire(parent, now, true).await;
        let prefix = fixture.logs().await;
        fixture
            .fail(1, doc! {"failCommands":["find"],"errorCode":2_i32})
            .await;
        let (entered, publishing) = tokio::sync::oneshot::channel();
        let (resume, release) = std::sync::mpsc::channel();
        *fixture.commit_session.lock().unwrap() = None;
        *fixture.publication_barrier.lock().unwrap() = Some((entered, release));
        let expiry = SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                layer2_core::seconds_from_rfc3339(&claim.work.lease.as_ref().unwrap().expires_at)
                    .unwrap(),
            );
        let remaining = Duration::from_secs(3);
        let worker = fixture.h.worker.clone();
        let attempt = claim.clone();
        let execution = tokio::spawn(async move {
            worker
                .execute_claimed_for_test(attempt, expiry - remaining)
                .await
        });
        tokio::time::timeout(remaining, publishing)
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(remaining).await;
        resume.send(()).unwrap();
        let result = execution.await.unwrap();
        fixture.clear_fault().await;
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert!(fixture.commit_session.lock().unwrap().is_none());
        assert_eq!(fixture.stored().await, claim);
        assert_eq!(fixture.logs().await, prefix);
        fixture.converge(&fixture.h.worker, expiry).await;
        fixture.admin.drop().await.unwrap();
    }
}
