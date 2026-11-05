use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn only_gateway_admin_can_verify_an_app_and_identity_cannot_be_claimed_twice() {
    let fixture = Fixture::new(true).await;
    let alice = fixture
        .register("token-alice-manager", "alice-draft", None)
        .await;
    let bob = fixture.register("token-bob", "bob-draft", None).await;
    let alice_ref = alice["app_ref"].as_str().unwrap();
    let bob_ref = bob["app_ref"].as_str().unwrap();
    let body = json!({"expected_version":1,"evidence_ref":"review:github-app-admin-confirmed"});
    let url = format!("{}/v1/github/apps/{alice_ref}/verification", fixture.base);
    let denied = fixture
        .client
        .post(&url)
        .bearer_auth("token-alice-manager")
        .header("Idempotency-Key", "verify-app")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let wrong_owner = fixture
        .client
        .post(&url)
        .bearer_auth("token-bob")
        .header("Idempotency-Key", "verify-app")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_owner.status(), 404);
    let response = fixture
        .client
        .post(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "verify-app")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let verified: serde_json::Value = response.json().await.unwrap();
    assert_eq!(verified["status"], "verified");
    assert_eq!(verified["version"], 2);
    assert_eq!(verified["verification"]["verified_by"], "alice");
    assert_eq!(verified["verification_credential"]["present"], false);
    let collision = fixture
        .client
        .post(format!(
            "{}/v1/github/apps/{bob_ref}/verification",
            fixture.base
        ))
        .bearer_auth("token-bob")
        .header("Idempotency-Key", "verify-app")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(collision.status(), 409);
    assert_eq!(
        fixture
            .read(
                &fixture.base,
                &format!("/v1/github/apps/{bob_ref}"),
                "token-bob"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        bob
    );
    let changed_identity = fixture
        .client
        .patch(format!("{}/v1/github/apps/{alice_ref}", fixture.base))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "replace-identity")
        .json(&json!({"expected_version":2,"display_name":"Replacement","app_id":99}))
        .send()
        .await
        .unwrap();
    assert_eq!(changed_identity.status(), 400);
    let activation = fixture
        .client
        .patch(format!("{}/v1/github/apps/{alice_ref}", fixture.base))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "activate-unchecked-app")
        .json(
            &json!({"expected_version":2,"display_name":"Premature activation","status":"active"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(activation.status(), 400);
    let replay = fixture
        .client
        .post(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "verify-app")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), verified);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn organization_scope_requires_current_membership_and_keeps_the_verifier_identity() {
    let fixture = Fixture::new(true).await;
    fixture
        .nyxid
        .oauth
        .grant("org-pilot", "Pilot", "alice", "admin");
    fixture
        .nyxid
        .oauth
        .grant("org-pilot", "Pilot", "bob", "member");
    let (alice, _) = fixture.nyxid.oauth.issue_tokens("alice");
    let (bob, _) = fixture.nyxid.oauth.issue_tokens("bob");
    let request = json!({"app_id":42,"display_name":"Organization integration"});
    let denied = fixture
        .registration(&fixture.base, &bob, "org-app", &request)
        .header("X-CMAEG-Owner", "org-pilot")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let created = fixture
        .registration(&fixture.base, &alice, "org-app", &request)
        .header("X-CMAEG-Owner", "org-pilot")
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: serde_json::Value = created.json().await.unwrap();
    let app_ref = created["app_ref"].as_str().unwrap();
    let path = format!("/v1/github/apps/{app_ref}");
    assert_eq!(
        fixture.read(&fixture.base, &path, &alice).await.status(),
        404
    );
    let scoped = fixture
        .client
        .get(format!("{}{path}", fixture.base))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-pilot")
        .send()
        .await
        .unwrap();
    assert_eq!(scoped.status(), 200);
    let verified = fixture
        .client
        .post(format!("{}{path}/verification", fixture.base))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-pilot")
        .header("Idempotency-Key", "verify-org-app")
        .json(&json!({"expected_version":1,"evidence_ref":"review:org-app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(verified.status(), 200);
    assert_eq!(
        verified.json::<serde_json::Value>().await.unwrap()["verification"]["verified_by"],
        "alice"
    );
    fixture
        .nyxid
        .oauth
        .grant("org-pilot", "Pilot", "alice", "member");
    let revoked = fixture
        .client
        .get(format!("{}{path}", fixture.base))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-pilot")
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 401);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn concurrent_app_verifications_reserve_only_one_identity_across_replicas() {
    let mut fixture = Fixture::new(true).await;
    let second = fixture.replica().await;
    let alice = fixture
        .register("token-alice", "verify-race-alice", None)
        .await;
    let bob = fixture.register("token-bob", "verify-race-bob", None).await;
    let body = json!({"expected_version":1,"evidence_ref":"review:concurrent-app"});
    let requests = [
        (&fixture.base, &alice, "token-alice"),
        (&second, &bob, "token-bob"),
    ];
    let pending: Vec<_> = requests
        .iter()
        .map(|(base, app, token)| {
            fixture
                .client
                .post(format!(
                    "{base}/v1/github/apps/{}/verification",
                    app["app_ref"].as_str().unwrap()
                ))
                .bearer_auth(token)
                .header("Idempotency-Key", "verify-once")
                .json(&body)
                .send()
        })
        .collect();
    let responses = futures_util::future::join_all(pending).await;
    let mut successful = 0;
    for (response, (base, app, token)) in responses.into_iter().zip(requests) {
        let mut response = response.unwrap();
        let app_ref = app["app_ref"].as_str().unwrap();
        if response.status() == 503 {
            response = fixture
                .client
                .post(format!("{base}/v1/github/apps/{app_ref}/verification"))
                .bearer_auth(token)
                .header("Idempotency-Key", "verify-once")
                .json(&body)
                .send()
                .await
                .unwrap();
        }
        let expected_status = match response.status().as_u16() {
            200 => {
                successful += 1;
                "verified"
            }
            409 => "pending_verification",
            other => panic!("unexpected verification status {other}"),
        };
        let metadata = fixture
            .read(base, &format!("/v1/github/apps/{app_ref}"), token)
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(metadata["status"], expected_status);
    }
    assert_eq!(successful, 1);
}
