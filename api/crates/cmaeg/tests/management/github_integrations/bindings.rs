use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn verified_owner_can_request_a_pending_installation_without_static_proof() {
    let fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "binding-app", None).await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let verified = fixture
        .client
        .post(format!(
            "{}/v1/github/apps/{app_ref}/verification",
            fixture.base
        ))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "verify-binding-app")
        .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(verified.status(), 200);
    let request = json!({"app_ref":app_ref,"installation_id":99,"account_id":123,"repository_ids":[1001,1002]});
    let created = fixture
        .client
        .post(format!("{}/v1/github/installation-bindings", fixture.base))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "binding-request")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let binding: serde_json::Value = created.json().await.unwrap();
    assert_eq!(binding["status"], "pending_verification");
    assert_eq!(binding["account_id"], 123);
    assert_eq!(binding["authority_revision"], 0);
    assert_eq!(binding["repository_ids"], json!([]));
    assert_eq!(binding["requested_repository_ids"], json!([1001, 1002]));
    let id = binding["binding_id"].as_str().unwrap();
    let read = fixture
        .read(
            &fixture.base,
            &format!("/v1/github/installation-bindings/{id}"),
            "token-alice",
        )
        .await;
    assert_eq!(read.status(), 200);
    assert_eq!(read.headers()["cache-control"], "no-store");
    assert_eq!(read.json::<serde_json::Value>().await.unwrap(), binding);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn binding_request_replays_after_restart_and_rejects_changed_input() {
    let mut fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "binding-app", None).await;
    let app_ref = app["app_ref"].as_str().unwrap();
    assert_eq!(
        fixture
            .client
            .post(format!(
                "{}/v1/github/apps/{app_ref}/verification",
                fixture.base
            ))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "verify")
            .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let body =
        json!({"app_ref":app_ref,"installation_id":99,"account_id":123,"repository_ids":[1001]});
    let post = |base: &str, body: serde_json::Value| {
        fixture
            .client
            .post(format!("{base}/v1/github/installation-bindings"))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "bind")
            .json(&body)
    };
    let first = post(&fixture.base, body.clone()).send().await.unwrap();
    assert_eq!(first.status(), 201);
    let original: serde_json::Value = first.json().await.unwrap();
    let mut changed_body = body.clone();
    changed_body["account_id"] = json!(456);
    let changed = post(&fixture.base, changed_body);
    assert_eq!(changed.send().await.unwrap().status(), 409);
    for child in &mut fixture.children {
        child.kill().await.unwrap();
    }
    let base = fixture.replica().await;
    let replay = fixture
        .client
        .post(format!("{base}/v1/github/installation-bindings"))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "bind")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), original);
}

pub(super) async fn pending(fixture: &Fixture) -> (serde_json::Value, serde_json::Value) {
    let app = fixture
        .register("token-alice", "app", Some("fixture-webhook-secret"))
        .await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let response = fixture
        .client
        .post(format!(
            "{}/v1/github/apps/{app_ref}/verification",
            fixture.base
        ))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "verify-app")
        .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let app = response.json::<serde_json::Value>().await.unwrap();
    let response = fixture.client.post(format!("{}/v1/github/installation-bindings", fixture.base))
        .bearer_auth("token-alice").header("Idempotency-Key", "binding")
        .json(&json!({"app_ref":app["app_ref"],"installation_id":99,"account_id":123,"repository_ids":[1001,1002]}))
        .send().await.unwrap();
    assert_eq!(response.status(), 201);
    (app, response.json().await.unwrap())
}

pub(super) fn verification(
    fixture: &Fixture,
    binding: &serde_json::Value,
    token: &str,
    key: &str,
) -> reqwest::RequestBuilder {
    fixture.client.post(format!("{}/v1/github/installation-bindings/{}/verification", fixture.base, binding["binding_id"].as_str().unwrap()))
        .bearer_auth(token).header("Idempotency-Key", key)
        .json(&json!({"expected_version":binding["version"],"evidence_ref":"review:installation-owner","repository_ids":[1001,1002]}))
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn administrator_approval_is_owner_scoped_versioned_and_audited() {
    let fixture = Fixture::new(true).await;
    let (_, binding) = pending(&fixture).await;
    assert_eq!(
        verification(&fixture, &binding, "token-alice-manager", "verify")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        verification(&fixture, &binding, "token-bob", "verify")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let response = verification(&fixture, &binding, "token-alice", "verify")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let approved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(approved["status"], "active");
    assert_eq!(approved["version"], 2);
    assert_eq!(approved["authority_revision"], 1);
    assert_eq!(approved["repository_ids"], json!([1001, 1002]));
    assert_eq!(approved["approval"]["verified_by"], "alice");
    assert_eq!(
        approved["approval"]["evidence_ref"],
        "review:installation-owner"
    );
    assert!(approved["approval"]["approved_at"].as_i64().unwrap() > 0);
    assert_eq!(
        verification(&fixture, &binding, "token-alice", "stale")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let replay = verification(&fixture, &binding, "token-alice", "verify")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), approved);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn eligible_business_delivery_without_a_matching_channel_is_a_terminal_receipt() {
    let fixture = Fixture::new(true).await;
    let (app, binding) = pending(&fixture).await;
    assert_eq!(
        verification(&fixture, &binding, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let app_ref = app["app_ref"].as_str().unwrap();
    let body = json!({"action":"opened","installation":{"id":99},"repository":{"id":1001,"full_name":"fixture/source","owner":{"id":123}},"issue":{"id":9001,"number":7,"html_url":"https://github.com/fixture/source/issues/7","body":"untrusted instructions"},"sender":{"login":"octocat","type":"User"}}).to_string();
    let url = format!("{}/v1/github/apps/{app_ref}/webhooks", fixture.base);
    for attempt in 0..2 {
        let response = fixture
            .webhook(
                &url,
                "fixture-webhook-secret",
                "business-1",
                "issues",
                &body,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let receipt = response.json::<serde_json::Value>().await.unwrap();
        assert_eq!(receipt["reason"], "no_matching_channel");
        let read = fixture
            .read(
                &fixture.base,
                &format!("/v1/github/apps/{app_ref}/deliveries/business-1"),
                "token-alice",
            )
            .await;
        assert_eq!(read.status(), 200);
        if attempt == 1 {
            assert_eq!(
                read.json::<serde_json::Value>().await.unwrap()["reason"],
                "no_matching_channel"
            );
        }
    }
    let capabilities = fixture
        .read(&fixture.base, "/v1/github/capabilities", "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(capabilities["ownership_verification"], true);
    assert_eq!(capabilities["durable_dispatch"], true);
    assert_eq!(capabilities["automatic_reconciliation"], false);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn unbound_and_pending_deliveries_are_terminal_ignored_receipts() {
    let fixture = Fixture::new(true).await;
    let (app, binding) = pending(&fixture).await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let url = format!("{}/v1/github/apps/{app_ref}/webhooks", fixture.base);
    for (installation, reason) in [
        (100, "installation_unbound"),
        (99, "installation_pending_verification"),
    ] {
        let body = json!({"action":"opened","installation":{"id":installation},"repository":{"id":1001,"owner":{"id":123}},"issue":{"id":9001,"number":7}}).to_string();
        let id = format!("ignored-{installation}");
        let response = fixture
            .webhook(&url, "fixture-webhook-secret", &id, "issues", &body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let receipt: serde_json::Value = response.json().await.unwrap();
        assert_eq!(receipt["status"], "ignored");
        assert_eq!(receipt["reason"], reason);
        assert_eq!(receipt["task_ids"], json!([]));
        assert_eq!(
            fixture
                .read(
                    &fixture.base,
                    &format!(
                        "/v1/github/deliveries/{}",
                        receipt["receipt_id"].as_str().unwrap()
                    ),
                    "token-bob"
                )
                .await
                .status(),
            404
        );
        if installation == 99 {
            assert_eq!(
                verification(&fixture, &binding, "token-alice", "approve")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
            let replay = fixture
                .webhook(&url, "fixture-webhook-secret", &id, "issues", &body)
                .send()
                .await
                .unwrap();
            assert_eq!(replay.status(), 200);
            assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), receipt);
        }
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn legacy_proof_request_has_a_typed_refusal_without_an_idempotency_header() {
    let f = Fixture::new(true).await;
    let app = f.register("token-alice", "legacy-shape", None).await;
    let response = f
        .client
        .post(format!("{}/v1/github/installation-bindings", f.base))
        .bearer_auth("token-alice")
        .json(&json!({"app_ref":app["app_ref"],"installation_id":99,"proof_ref":"withdrawn-proof"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"],
        "github_proof_withdrawn"
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn binding_scope_is_bounded_and_the_maximum_scope_can_be_approved() {
    let f = Fixture::new(true).await;
    let (app, _) = pending(&f).await;
    let ids: Vec<u64> = (0..257).map(|offset| u64::MAX - offset).collect();
    let request = |key: &str, repositories: &[u64]| {
        f.client.post(format!("{}/v1/github/installation-bindings",f.base))
        .bearer_auth("token-alice").header("Idempotency-Key",key)
        .json(&json!({"app_ref":app["app_ref"],"installation_id":100,"account_id":123,"repository_ids":repositories}))
    };
    assert_eq!(
        request("oversized", &ids).send().await.unwrap().status(),
        400
    );
    let created = request("bounded", &ids[..256]).send().await.unwrap();
    assert_eq!(created.status(), 201);
    let binding: serde_json::Value = created.json().await.unwrap();
    let approved=f.client.post(format!("{}/v1/github/installation-bindings/{}/verification",f.base,binding["binding_id"].as_str().unwrap()))
        .bearer_auth("token-alice").header("Idempotency-Key","approve-bounded")
        .json(&json!({"expected_version":1,"evidence_ref":"review:bounded","repository_ids":&ids[..256]})).send().await.unwrap();
    assert_eq!(approved.status(), 200);
    assert_eq!(
        approved.json::<serde_json::Value>().await.unwrap()["repository_ids"],
        json!(&ids[..256])
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn administrator_approved_scope_is_authoritative_even_when_it_differs_from_the_request() {
    let f = Fixture::new(true).await;
    let (_, binding) = pending(&f).await;
    let response=f.client.post(format!("{}/v1/github/installation-bindings/{}/verification",f.base,binding["binding_id"].as_str().unwrap()))
        .bearer_auth("token-alice").header("Idempotency-Key","independent-scope")
        .json(&json!({"expected_version":1,"evidence_ref":"review:independent-repository-scope","repository_ids":[2001]})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let approved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(approved["requested_repository_ids"], json!([1001, 1002]));
    assert_eq!(approved["approved_repository_ids"], json!([2001]));
    assert_eq!(approved["repository_ids"], json!([2001]));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn reapproving_unchanged_eligibility_changes_audit_version_only() {
    let f = Fixture::new(true).await;
    let (_, binding) = pending(&f).await;
    let approved = verification(&f, &binding, "token-alice", "approve")
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let response = verification(&f, &approved, "token-alice", "refresh-audit")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let repeated: serde_json::Value = response.json().await.unwrap();
    assert_eq!(repeated["version"], 3);
    assert_eq!(repeated["authority_revision"], 1);
}
