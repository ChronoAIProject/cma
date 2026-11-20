use super::*;
use std::sync::atomic::Ordering;

async fn start_teardown(f: &Fixture) {
    let mut row = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    let revision = row.revision;
    row.work.step = SandboxWorkStep::TeardownRuntime;
    row.revision += 1;
    f.store.save_sandbox(revision, row).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_retains_unresolved_issuance_before_and_after_late_result() {
    let mut f = Fixture::new("retirement_unknown_mint").await;
    let issuing = f.issue(1);
    let mint = f.prepare().await;
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    let (worker, _) = retirement_fixture::begin(&f).await;
    for _ in 0..2 {
        assert!(worker
            .run_one(&f.claim.id, SystemTime::now())
            .await
            .unwrap());
    }
    let blocked = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(blocked.status, SandboxStatus::Retiring);
    assert_eq!(
        blocked.work.step,
        SandboxWorkStep::RevokeRetirementCredentials
    );
    assert!(blocked.purge_after.is_none());
    assert!(!worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    accepted(mint);
    assert!(!matches!(issuing.await.unwrap(), Ok(Ok(_))));
    assert_eq!(f.custody.issuance(&blocked).await.unwrap(), Some(intent));
    assert!(f.custody.list(&f.claim.id).await.unwrap().is_empty());
    for step in [
        SandboxWorkStep::TeardownRuntime,
        SandboxWorkStep::MarkRetired,
    ] {
        let mut row = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
        let revision = row.revision;
        row.work.step = step;
        row.work.ready_at = at(-1);
        row.revision += 1;
        f.store.save_sandbox(revision, row).await.unwrap();
        assert!(worker
            .run_one(&f.claim.id, SystemTime::now())
            .await
            .unwrap());
        let blocked = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
        assert_eq!(blocked.status, SandboxStatus::Retiring);
        assert_eq!(blocked.work.step, step);
        assert!(blocked.purge_after.is_none());
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_settles_independent_queued_debt_before_teardown() {
    let mut f = Fixture::new("retirement_queued_debt").await;
    let (worker, _) = retirement_fixture::begin(&f).await;
    start_teardown(&f).await;
    f.custody
        .queue_revocation(crate::sandbox::CredentialRevocation {
            sandbox_id: f.claim.id.clone(),
            owner: f.claim.owner.clone(),
            provider_key_id: "queued-key".into(),
        })
        .await
        .unwrap();
    for fail in [true, false] {
        let id = f.claim.id.clone();
        let runner = worker.clone();
        let running = tokio::spawn(async move { runner.run_one(&id, SystemTime::now()).await });
        let request = f.peer.next().await;
        assert_eq!(request.uri.path(), "/api/v1/api-keys/queued-key");
        request.respond(if fail { 503 } else { 204 }, "");
        running.await.unwrap().unwrap();
        let mut row = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
        assert_eq!(
            row.work.step,
            if fail {
                SandboxWorkStep::TeardownRuntime
            } else {
                SandboxWorkStep::MarkRetired
            }
        );
        assert_eq!(
            f.custody.revocations(&f.claim.id).await.unwrap().len(),
            usize::from(fail)
        );
        if fail {
            assert!(row.credential.revoke_debt.is_empty());
            assert!(row.purge_after.is_none());
            assert!(!worker
                .run_one(&f.claim.id, SystemTime::now())
                .await
                .unwrap());
            let revision = row.revision;
            row.work.ready_at = at(-1);
            row.revision += 1;
            f.store.save_sandbox(revision, row).await.unwrap();
        }
    }
    assert!(worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    assert_eq!(
        f.store
            .get_sandbox(&f.claim.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Retired
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_census_cannot_authorize_cleanup_after_claim_takeover() {
    let f = Fixture::new("retirement_stale_gate").await;
    let (worker, scans) = retirement_fixture::begin(&f).await;
    start_teardown(&f).await;
    scans.hold_at.store(2, Ordering::SeqCst);
    let id = f.claim.id.clone();
    let running = tokio::spawn(async move { worker.run_one(&id, SystemTime::now()).await });
    scans.entered.notified().await;
    let independent = peer(&f.name).await;
    Kind::Sandbox
        .set(
            &independent,
            doc! {"work_lease_expires_at":at(-1),"record.work.lease.expires_at":at(-1)},
        )
        .await;
    let successor = independent
        .claim_sandbox_work(&f.claim.id, "successor", at(0), at(15))
        .await
        .unwrap()
        .unwrap();
    scans.resume.notify_one();
    assert!(running.await.unwrap().is_err());
    assert_eq!(
        independent.get_sandbox(&f.claim.id).await.unwrap().unwrap(),
        successor
    );
    assert_eq!(successor.work.step, SandboxWorkStep::TeardownRuntime);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn retirement_unknown_settlement_commit_holds_cleanup_until_successor_rechecks() {
    let f = Fixture::new("retirement_unknown_gate").await;
    let app = format!("retirement-gate-{}", uuid::Uuid::new_v4());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, &f.name)
            .await
            .unwrap()
            .with_renewable_work(true),
    );
    let (worker, scans) = retirement_fixture::begin_with_store(&f, store).await;
    start_teardown(&f).await;
    scans.hold_at.store(2, Ordering::SeqCst);
    let id = f.claim.id.clone();
    let runner = worker.clone();
    let running = tokio::spawn(async move { runner.run_one(&id, SystemTime::now()).await });
    scans.entered.notified().await;
    let before = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    let events = f.store.sandbox_events(&f.claim.id, 0, 100).await.unwrap();
    f.commit_failpoint(&app, 100).await;
    scans.resume.notify_one();
    let result = running.await.unwrap();
    f.clear_failpoint().await;
    assert!(result.is_err());
    let after = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(
        after.revision,
        before.revision + 1,
        "settlement committed despite lost ACK"
    );
    assert_eq!(after.work.step, SandboxWorkStep::TeardownRuntime);
    assert_eq!(after.status, SandboxStatus::Retiring);
    assert_eq!(
        f.store.sandbox_events(&f.claim.id, 0, 100).await.unwrap(),
        events
    );
    Kind::Sandbox
        .set(
            &f.store,
            doc! {"work_lease_expires_at":at(-1),"record.work.lease.expires_at":at(-1)},
        )
        .await;
    assert!(worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    assert_eq!(
        f.store
            .get_sandbox(&f.claim.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .step,
        SandboxWorkStep::MarkRetired
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_missing_intent_behind_issuance_marker_cannot_clear_obligation() {
    let f = Fixture::new("retirement_missing_intent").await;
    let (worker, _) = retirement_fixture::begin(&f).await;
    Kind::Sandbox
        .set(&f.store, doc! {"credential_issuance_fence":1_i64})
        .await;
    for _ in 0..2 {
        assert!(worker
            .run_one(&f.claim.id, SystemTime::now())
            .await
            .unwrap());
    }
    let row = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(row.status, SandboxStatus::Retiring);
    assert_eq!(row.work.step, SandboxWorkStep::RevokeRetirementCredentials);
    assert!(row.purge_after.is_none());
    f.cleanup().await;
}
