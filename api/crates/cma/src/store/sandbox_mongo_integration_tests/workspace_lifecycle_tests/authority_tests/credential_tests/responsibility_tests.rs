use super::legacy_admission_tests::{independent_custody, retained};
use super::*;
use crate::sandbox::{CredentialRevocation, IssuanceState};

async fn reject_command(f: &Fixture, app: &str, command: &str) -> i32 {
    f.client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"alwaysOn",
            "data":{"failCommands":[command], "appName":app, "errorCode":2_i32}
        })
        .await
        .unwrap()
        .get_i32("count")
        .unwrap()
}

async fn finish_rejection(f: &Fixture, before: i32) {
    let result = f
        .client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert!(result.get_i32("count").unwrap() > before);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn accepted_result_survives_confirmed_database_abort_opening_custody() {
    let mut f = Fixture::new("issuance_acceptance_write_conflict").await;
    let app = format!("acceptance-conflict-{}", uuid::Uuid::new_v4());
    f.use_custody_app(&app).await;
    let issuing = f.issue(1);
    let mint = f.prepare().await;
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();

    let before = f
        .client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["update"], "appName":app, "errorCode":112_i32,
                "errorLabels":["TransientTransactionError"]}
        })
        .await
        .unwrap()
        .get_i32("count")
        .unwrap();
    accepted(mint);
    let result = issuing.await.unwrap().unwrap();
    finish_rejection(&f, before).await;

    let material = result.unwrap();
    let stored = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    assert_eq!(stored.state, IssuanceState::Accepted);
    assert_eq!(stored.operation_id, intent.operation_id);
    assert_eq!(stored.original_fence, intent.original_fence);
    let custody = f.custody.list(&f.claim.id).await.unwrap();
    assert_eq!(custody.len(), 1);
    assert_eq!(custody[0].authority, material.authority);
    assert!(f.controller.settle_revocations(&f.claim).await.is_ok());
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn rejected_issuance_barrier_cannot_dispatch_an_unowned_key() {
    let mut f = Fixture::new("issuance_barrier_rejected").await;
    let app = format!("barrier-rejected-{}", uuid::Uuid::new_v4());
    f.use_custody_app(&app).await;
    let before = reject_command(&f, &app, "update").await;
    let issuing = f.issue(1);
    acknowledgements::prepare_only(&mut f).await;
    let result = issuing.await.unwrap().unwrap();
    finish_rejection(&f, before).await;
    assert!(matches!(result, Err(CredentialError::Unavailable(_))));
    assert!(f.custody.issuance(&f.claim).await.unwrap().is_none());
    assert!(f.custody.list(&f.claim.id).await.unwrap().is_empty());
    assert_eq!(f.peer.count(), 2, "only preparation reached the provider");

    f.takeover().await;
    let retry = f.issue(1);
    let mint = f.prepare().await;
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    let body: Value = serde_json::from_slice(&mint.body).unwrap();
    assert_eq!(
        body["name"],
        format!("cma:work-contract:{}", intent.operation_id)
    );
    assert_eq!(intent.state, IssuanceState::Unknown);
    accepted(mint);
    assert!(retry.await.unwrap().unwrap().is_ok());
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn rejected_result_write_keeps_discoverable_responsibility_after_return() {
    let mut f = Fixture::new("issuance_result_rejected").await;
    let app = format!("result-rejected-{}", uuid::Uuid::new_v4());
    f.use_custody_app(&app).await;
    let issuing = f.issue(1);
    let mint = f.prepare().await;
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    let body: Value = serde_json::from_slice(&mint.body).unwrap();
    assert_eq!(
        body["name"],
        format!("cma:work-contract:{}", intent.operation_id)
    );
    let before = reject_command(&f, &app, "insert").await;
    accepted(mint);
    let result = issuing.await.unwrap().unwrap();
    finish_rejection(&f, before).await;
    assert!(matches!(result, Err(CredentialError::Uncertain)));
    assert_eq!(
        f.custody.issuance(&f.claim).await.unwrap(),
        Some(intent.clone())
    );
    assert!(f.custody.list(&f.claim.id).await.unwrap().is_empty());

    f.takeover().await;
    assert_eq!(
        f.custody.issuance(&f.claim).await.unwrap(),
        Some(intent.clone())
    );
    assert!(matches!(
        f.issue(1).await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    f.due_observation().await;
    let observing = f.issue(1);
    let request = f.peer.next().await;
    assert_eq!(request.method, axum::http::Method::GET);
    assert_eq!(request.uri.path(), "/api/v1/api-keys");
    request.respond(503, "fixture observation unavailable");
    assert!(matches!(
        observing.await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    let unresolved = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    assert_eq!(unresolved.operation_id, intent.operation_id);
    assert_eq!(unresolved.state, IssuanceState::Unknown);
    assert_eq!(unresolved.original_fence, intent.original_fence);

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
    assert!(f
        .store
        .due_sandbox_work(&blocked.work.ready_at, 10)
        .await
        .unwrap()
        .contains(&blocked.id));
    assert_eq!(
        f.custody.issuance(&blocked).await.unwrap(),
        Some(unresolved)
    );
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
    assert_eq!(f.peer.count(), 4, "no compensating revoke or new mint");
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn conflicting_retained_result_cannot_replace_the_inflight_key_obligation() {
    let mut f = Fixture::new("issuance_result_conflict").await;
    let issuing = f.issue(1);
    let mint = f.prepare().await;
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    // Model retained custody arriving after dispatch. Current generic writers
    // refuse managed material; this fixture does not authorize an old writer.
    let winner = retained(&f);
    f.custody.seed_retained_for_test(&winner).await;
    accepted(mint);
    assert!(matches!(
        issuing.await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    assert_eq!(
        f.custody.issuance(&f.claim).await.unwrap(),
        Some(intent.clone())
    );

    f.takeover().await;
    assert!(matches!(
        f.issue(1).await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    assert!(matches!(
        f.controller.settle_revocations(&f.claim).await,
        Err(CredentialError::Uncertain)
    ));
    let stored = f.custody.get(&f.claim.id, 1).await.unwrap().unwrap();
    assert_eq!(stored.authority, winner.authority);
    assert_eq!(stored.value.expose(), winner.value.expose());
    assert_eq!(f.custody.issuance(&f.claim).await.unwrap(), Some(intent));
    assert_eq!(
        f.peer.count(),
        3,
        "neither key may be blindly revoked or replaced"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn retained_loser_debt_survives_revoke_and_delete_failures_across_restart() {
    let mut f = Fixture::new_mode("retained_loser_failures", false).await;
    let winner = retained(&f);
    f.custody.seed_retained_for_test(&winner).await;
    let loser = CredentialRevocation {
        sandbox_id: f.claim.id.clone(),
        owner: f.claim.owner.clone(),
        provider_key_id: "retained-loser-key".into(),
    };
    let unrelated = CredentialRevocation {
        sandbox_id: SandboxId::generate(),
        owner: SubjectId::new("unrelated-owner"),
        provider_key_id: loser.provider_key_id.clone(),
    };
    for debt in [&loser, &unrelated] {
        f.custody.queue_revocation(debt.clone()).await.unwrap();
    }
    let observer = independent_custody(&f).await;
    for attempt in 0..3 {
        let app = format!("loser-recovery-{}", uuid::Uuid::new_v4());
        f.use_custody_app(&app).await;
        let before = if attempt == 1 {
            Some(reject_command(&f, &app, "delete").await)
        } else {
            None
        };
        let controller = f.controller.clone();
        let claim = f.claim.clone();
        let recovering =
            tokio::spawn(async move { controller.issue(&claim, 1, SystemTime::now()).await });
        let request = f.peer.next().await;
        assert_eq!(request.method, axum::http::Method::DELETE);
        assert_eq!(request.uri.path(), "/api/v1/api-keys/retained-loser-key");
        request.respond(if attempt == 0 { 503 } else { 204 }, "");
        let result = recovering.await.unwrap();
        if let Some(before) = before {
            finish_rejection(&f, before).await;
        }
        let debts = observer.revocations(&f.claim.id).await.unwrap();
        if attempt < 2 {
            assert!(matches!(result, Err(CredentialError::Unavailable(_))));
            assert_eq!(debts.as_slice(), std::slice::from_ref(&loser));
        } else {
            let recovered = result.unwrap();
            assert_eq!(recovered.authority, winner.authority);
            assert_eq!(recovered.value.expose(), winner.value.expose());
            assert!(debts.is_empty());
        }
        assert_eq!(
            observer
                .revocations(&unrelated.sandbox_id)
                .await
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&unrelated)
        );
        let stored = observer.get(&f.claim.id, 1).await.unwrap().unwrap();
        assert_eq!(stored.authority, winner.authority);
        assert_eq!(stored.value.expose(), winner.value.expose());
    }
    assert_eq!(
        f.peer
            .count_route("DELETE", "/api/v1/api-keys/retained-loser-key"),
        3
    );
    assert_eq!(f.peer.count(), 3);
    f.cleanup().await;
}
