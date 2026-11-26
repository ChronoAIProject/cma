use super::*;

async fn uncertain(fixture: &mut Fixture) {
    let original = fixture.issue(1);
    fixture.prepare().await.stall_body();
    original.abort();
    assert!(original.await.unwrap_err().is_cancelled());
    fixture.takeover().await;
    fixture.due_observation().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_reconcilers_preserve_one_unknown_obligation() {
    let mut fixture = Fixture::new("mint_observation_race").await;
    uncertain(&mut fixture).await;
    let first = fixture.issue(1);
    let first_read = fixture.peer.next().await;
    assert_eq!(first_read.method, axum::http::Method::GET);
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let custody = Arc::new(
        MongoSandboxCredentialStore::open(&client, &fixture.name, "work-contract", &fixture.key)
            .await
            .unwrap(),
    );
    let controller =
        ManagedSandboxCredentials::new(fixture.issuer.clone(), Arc::new(Tokens), custody);
    let independent = MongoSandboxStore::open(&client, &fixture.name)
        .await
        .unwrap();
    let second = fixture.issue_with(independent, controller, 9);
    let second_read = fixture.peer.next().await;
    assert_eq!(second_read.method, axum::http::Method::GET);
    assert_eq!(second_read.uri.path(), "/api/v1/api-keys");
    first_read.json(json!([]));
    assert!(matches!(
        first.await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    // Release the second response after the first observation has moved the
    // durable due time. A stale observation cannot erase or replace custody.
    second_read.json(json!([]));
    assert!(matches!(
        second.await.unwrap().unwrap(),
        Err(CredentialError::Unavailable(_))
    ));
    let intent = fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, crate::sandbox::IssuanceState::Unknown);
    assert_eq!(intent.generation, 1);
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
    assert_eq!(fixture.peer.count_route("GET", "/api/v1/api-keys"), 2);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provider_change_and_unavailable_observation_never_authorize_mint() {
    let mut fixture = Fixture::new("mint_observation_failure").await;
    uncertain(&mut fixture).await;
    for (status, body) in [(503, "unavailable"), (200, "malformed")] {
        fixture.due_observation().await;
        let checking = fixture.issue(1);
        let request = fixture.peer.next().await;
        assert_eq!(request.method, axum::http::Method::GET);
        request.respond(status, body);
        assert!(matches!(
            checking.await.unwrap().unwrap(),
            Err(CredentialError::Uncertain)
        ));
    }
    let replacement = Peer::start().await;
    let issuer = Arc::new(KeyIssuer::new(
        reqwest::Client::new(),
        &replacement.origin,
        "work-contract",
    ));
    let controller =
        ManagedSandboxCredentials::new(issuer, Arc::new(Tokens), fixture.custody.clone());
    fixture.due_observation().await;
    assert!(matches!(
        fixture
            .issue_with(fixture.store.clone(), controller, 2)
            .await
            .unwrap()
            .unwrap(),
        Err(CredentialError::Uncertain)
    ));
    assert_eq!(replacement.count(), 0);
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
    let raw = fixture
        .client
        .database(&fixture.name)
        .collection::<Document>("sandbox_credential_issuance")
        .find_one(doc! {"_id":fixture.claim.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        raw["last_observation"]["kind"].as_str(),
        Some("unavailable")
    );
    assert_eq!(raw["record"]["state"].as_str(), Some("unknown"));
    assert!(raw["record"]["next_check_at"].as_str().unwrap() > at(0).as_str());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn successor_cannot_store_a_result_using_predecessor_dispatch_custody() {
    let mut fixture = Fixture::new("mint_original_result_authority").await;
    uncertain(&mut fixture).await;
    let intent = fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .unwrap();
    let material = CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: fixture.claim.id.clone(),
        owner: fixture.claim.owner.clone(),
        authority: layer2_core::sandbox::SandboxCredentialAuthority {
            generation: intent.generation,
            provider_key_id: "fixture-provider-key".into(),
            provider_state_version: 7,
            issued_at: intent.created_at.clone(),
            expires_at: at(3600),
            grant_fingerprint: "a".repeat(64),
        },
        value: SecretValue::new(format!("fixture_{}", uuid::Uuid::new_v4())),
    };
    assert_ne!(
        fixture.claim.work.lease.as_ref().unwrap().fence,
        intent.original_fence
    );
    assert!(fixture
        .custody
        .accept_issuance(&fixture.claim, &intent, &material)
        .await
        .is_err());
    assert_eq!(
        fixture.custody.issuance(&fixture.claim).await.unwrap(),
        Some(intent)
    );
    assert!(fixture
        .custody
        .get(&fixture.claim.id, 1)
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
    fixture.cleanup().await;
}
