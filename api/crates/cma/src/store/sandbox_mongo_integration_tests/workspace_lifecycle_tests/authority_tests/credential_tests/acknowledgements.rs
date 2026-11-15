use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn committed_intent_with_lost_ack_never_dispatches_after_restart() {
    let mut fixture = Fixture::new("mint_intent_lost_ack").await;
    let app = format!("mint-intent-{}", uuid::Uuid::new_v4());
    fixture.use_custody_app(&app).await;
    fixture.commit_failpoint(&app, 100).await;
    let original = fixture.issue(1);
    prepare_only(&mut fixture).await;
    let result = original.await.unwrap().unwrap();
    fixture.clear_failpoint().await;
    assert!(matches!(result, Err(CredentialError::Unavailable(_))));
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 0);
    assert!(fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .is_some());
    fixture.takeover().await;
    assert!(matches!(
        fixture.issue(1).await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 0);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn death_during_intent_ack_retains_zero_send_barrier() {
    let mut fixture = Fixture::new("mint_intent_crash").await;
    let app = format!("mint-crash-{}", uuid::Uuid::new_v4());
    fixture.use_custody_app(&app).await;
    fixture.commit_failpoint(&app, 800).await;
    let original = fixture.issue(1);
    prepare_only(&mut fixture).await;
    // The independent observer waits for actual durable intent, not a sleep
    // approximating dispatch. The originating request has no committed receipt.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if fixture
                .custody
                .issuance(&fixture.claim)
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    original.abort();
    assert!(original.await.unwrap_err().is_cancelled());
    fixture.clear_failpoint().await;
    fixture.takeover().await;
    assert!(matches!(
        fixture.issue(1).await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 0);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn lost_result_commit_ack_reconciles_exact_encrypted_material() {
    let mut fixture = Fixture::new("mint_result_lost_ack").await;
    let app = format!("mint-result-{}", uuid::Uuid::new_v4());
    fixture.use_custody_app(&app).await;
    let original = fixture.issue(1);
    let request = fixture.prepare().await;
    // Intent has already committed. Only accepted-result storage loses its ACK.
    fixture.commit_failpoint(&app, 100).await;
    let secret = accepted(request);
    let returned = original.await.unwrap().unwrap();
    fixture.clear_failpoint().await;
    assert_eq!(returned.unwrap().value.expose(), secret);
    fixture.takeover().await;
    assert_eq!(
        fixture
            .issue(1)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .value
            .expose(),
        secret
    );
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
    fixture.cleanup().await;
}

pub(super) async fn prepare_only(fixture: &mut Fixture) {
    let services = fixture.peer.next().await;
    assert_eq!(services.uri.path(), "/api/v1/user-services");
    services.json(json!([{"id":"fixture-service-id","slug":"fixture-service","is_active":true}]));
    let scope = fixture.peer.next().await;
    assert_eq!(scope.uri.path(), "/api/v1/api-keys/scope-plan");
    let body: Value = serde_json::from_slice(&scope.body).unwrap();
    scope.json(crate::auth::keys::scope_plan_fixture(
        fixture.claim.owner.as_str(),
        &body,
        &["fixture-node"],
    ));
}
