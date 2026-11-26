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
        let expiry = claim_expiry(&claim);
        let remaining = Duration::from_secs(3);
        assert_expired_retry_is_fenced(&fixture, claim, expiry - remaining, async {
            tokio::time::sleep(remaining).await;
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn cleanup_preflight_retry_caps_stale_scheduler_time_at_the_stored_expiry() {
    let fixture = Fixture::new(false).await;
    fixture.delete("original-delete").await;
    let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
    let parent = fixture.begin_retirement(now).await;
    fixture.retire(parent, now, true).await;

    let scheduler_time = SystemTime::now().max(now);
    let lease = Duration::from_secs(8);
    // Ownership and admission can delay the claim while the scheduler's
    // timestamp still supplies its durable expiry.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let claim = fixture
        .h
        .store
        .claim_agent_work(
            &fixture.agent.id,
            "agent-worker",
            rfc3339_at(scheduler_time),
            rfc3339_at(scheduler_time + lease),
        )
        .await
        .unwrap()
        .unwrap();
    let expiry = claim_expiry(&claim);
    let stale_cutoff = tokio::time::Instant::now() + expiry.duration_since(scheduler_time).unwrap();
    assert_expired_retry_is_fenced(&fixture, claim, scheduler_time, async {
        let remaining = expiry.duration_since(SystemTime::now()).unwrap_or_default();
        tokio::time::sleep(remaining + Duration::from_millis(50)).await;
        assert!(SystemTime::now() > expiry);
        assert!(tokio::time::Instant::now() < stale_cutoff);
    })
    .await;
}

fn claim_expiry(claim: &Agent) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(
            layer2_core::seconds_from_rfc3339(&claim.work.lease.as_ref().unwrap().expires_at)
                .unwrap(),
        )
}

async fn assert_expired_retry_is_fenced(
    fixture: &Fixture,
    claim: Agent,
    now: SystemTime,
    hold_writer: impl std::future::Future<Output = ()>,
) {
    let prefix = fixture.logs().await;
    fixture
        .fail(1, doc! {"failCommands":["find"],"errorCode":2_i32})
        .await;
    let (entered, publishing) = tokio::sync::oneshot::channel();
    let (resume, release) = std::sync::mpsc::channel();
    *fixture.commit_session.lock().unwrap() = None;
    *fixture.publication_barrier.lock().unwrap() = Some((entered, release));
    let worker = fixture.h.worker.clone();
    let attempt = claim.clone();
    let execution =
        tokio::spawn(async move { worker.execute_claimed_for_test(attempt, now).await });
    tokio::time::timeout(Duration::from_secs(3), publishing)
        .await
        .unwrap()
        .unwrap();
    hold_writer.await;
    resume.send(()).unwrap();
    let result = execution.await.unwrap();
    fixture.clear_fault().await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert!(fixture.commit_session.lock().unwrap().is_none());
    assert_eq!(fixture.stored().await, claim);
    assert_eq!(fixture.logs().await, prefix);
    fixture
        .converge(&fixture.h.worker, claim_expiry(&claim))
        .await;
    fixture.admin.drop().await.unwrap();
}
