use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn image_origin_allows_fenced_credential_issuance_and_replay() {
    use layer2_core::sandbox::SandboxRuntimeImageOrigin::*;

    for origin in [ManagedDesktop, UserSelected, ManagedMachine] {
        let mut fixture =
            Fixture::new_with_image_origin("mint_image_origin", true, Some(origin)).await;
        let mut issuing = fixture.issue(1);
        let mint = tokio::select! {
            mint = fixture.prepare() => mint,
            outcome = &mut issuing => panic!("{origin:?} issuance ended before dispatch: {outcome:?}"),
        };
        let intent = fixture
            .custody
            .issuance(&fixture.claim)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(intent.state, crate::sandbox::IssuanceState::Unknown);
        accepted(mint);
        let issued = issuing.await.unwrap().unwrap().unwrap();
        assert_eq!(issued.owner, fixture.claim.owner);
        assert_eq!(issued.authority.generation, 1);

        fixture.takeover().await;
        let replay = fixture.issue(1).await.unwrap().unwrap().unwrap();
        assert_eq!(replay.authority, issued.authority);
        assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn accepted_mint_lost_response_remains_unknown_through_takeover_and_retries() {
    let mut fixture = Fixture::new("mint_unknown_takeover").await;
    let original = fixture.issue(1);
    let request = fixture.prepare().await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let name = body["name"].as_str().unwrap().to_owned();
    // Independent receiver ledger: one POST was accepted, but no result body
    // reaches the original. Dropping local work cannot undo that acceptance.
    request.stall_body();
    original.abort();
    assert!(original.await.unwrap_err().is_cancelled());
    fixture.takeover().await;
    for generation in [1, 2, 7, 1] {
        assert!(matches!(
            fixture.issue(generation).await.unwrap().unwrap(),
            Err(CredentialError::Uncertain)
        ));
    }
    assert_eq!(fixture.peer.count(), 3);
    for listing in [
        json!([]),
        json!([{"id":"fixture-provider-key","name":&name}]),
        json!([{"id":"fixture-provider-key","name":&name,"revoked":true}]),
    ] {
        fixture.due_observation().await;
        let checking = fixture.issue(1);
        let request = fixture.peer.next().await;
        assert_eq!(request.method, axum::http::Method::GET);
        assert_eq!(request.uri.path(), "/api/v1/api-keys");
        request.json(listing);
        assert!(matches!(
            checking.await.unwrap().unwrap(),
            Err(CredentialError::Uncertain)
        ));
    }
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
    assert_eq!(fixture.peer.count(), 6);
    let intent = fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, crate::sandbox::IssuanceState::Unknown);
    assert_eq!(intent.generation, 1);
    assert!(fixture
        .custody
        .get(&fixture.claim.id, 1)
        .await
        .unwrap()
        .is_none());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_accepted_material_replays_after_caller_loses_the_result() {
    let mut fixture = Fixture::new("mint_result_replay").await;
    let original = fixture.issue(1);
    let request = fixture.prepare().await;
    let secret = accepted(request);
    let returned = original.await.unwrap().unwrap().unwrap();
    assert_eq!(returned.value.expose(), secret);
    drop(returned); // Model a client losing the acknowledged result.
    fixture.takeover().await;
    let replay = fixture.issue(1).await.unwrap().unwrap().unwrap();
    assert_eq!(replay.value.expose(), secret);
    assert_eq!(fixture.peer.count(), 3);
    let intent = fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, crate::sandbox::IssuanceState::Accepted);
    let raw = fixture
        .client
        .database(&fixture.name)
        .collection::<Document>("sandbox_credentials")
        .find_one(doc! {"sandbox_id":fixture.claim.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{raw:?}").contains(&secret));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn proven_preparation_refusal_remains_retryable_without_an_intent() {
    let mut fixture = Fixture::new("mint_preparation_refusal").await;
    let first = fixture.issue(1);
    let services = fixture.peer.next().await;
    assert_eq!(services.uri.path(), "/api/v1/user-services");
    services.respond(503, "fixture service catalog unavailable");
    assert!(matches!(
        first.await.unwrap().unwrap(),
        Err(CredentialError::Unavailable(_))
    ));
    assert!(fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .is_none());
    let retry = fixture.issue(1);
    let request = fixture.prepare().await;
    accepted(request);
    assert!(retry.await.unwrap().unwrap().is_ok());
    assert_eq!(fixture.peer.count(), 4);
    fixture.cleanup().await;
}
