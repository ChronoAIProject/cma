use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn ping_authenticates_exact_bytes_before_replay_and_recovers_after_restart() {
    let mut fixture = Fixture::new(true).await;
    let secret = "synthetic-raw-body-ping-secret";
    let app = fixture
        .register("token-alice", "raw-ping", Some(secret))
        .await;
    let url = app["webhook_url"].as_str().unwrap();
    let app_ref = app["app_ref"].as_str().unwrap();
    let body = r#"{"zen":"Do one thing well","hook_id":9,"hook":{}}"#;
    let changed = format!("{body}\n");
    let response = fixture
        .webhook(url, secret, "raw-ping", "ping", body)
        .body(changed.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"],
        "github_signature_invalid"
    );
    let lookup = format!("/v1/github/apps/{app_ref}/deliveries/raw-ping");
    assert_eq!(
        fixture
            .read(&fixture.base, &lookup, "token-alice")
            .await
            .status(),
        404
    );
    let response = fixture
        .webhook(url, secret, "raw-ping", "ping", body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let receipt: serde_json::Value = response.json().await.unwrap();
    for (signature_secret, event, bytes, expected) in [
        ("wrong-signature-secret", "ping", body, 401),
        (secret, "ping", changed.as_str(), 409),
        (secret, "issues", body, 409),
    ] {
        let response = fixture
            .webhook(url, signature_secret, "raw-ping", event, bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    fixture.children[0].kill().await.unwrap();
    let restarted = fixture.replica().await;
    let replay = fixture
        .webhook(
            &format!("{restarted}/v1/github/apps/{app_ref}/webhooks"),
            secret,
            "raw-ping",
            "ping",
            body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), receipt);
    assert_eq!(
        fixture
            .read(&restarted, &lookup, "token-alice")
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        fixture
            .read(&restarted, &lookup, "token-bob")
            .await
            .status(),
        404
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn dynamic_app_ping_is_durable_and_queryable_across_running_replicas() {
    let mut fixture = Fixture::new(true).await;
    let second = fixture.replica().await;
    let secret = "synthetic-dynamic-ping-secret";
    let response = fixture
        .registration(
            &fixture.base,
            "token-alice",
            "register-ping",
            &json!({"app_id":42,"display_name":"Ping","webhook_secret":secret}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let app: serde_json::Value = response.json().await.unwrap();
    let app_ref = app["app_ref"].as_str().unwrap();
    let body = "{\n  \"zen\": \"保持简单\", \"hook_id\": 123, \"hook\": {}\n}";
    let response = fixture
        .webhook(
            app["webhook_url"].as_str().unwrap(),
            secret,
            "ping-001",
            "ping",
            body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let receipt: serde_json::Value = response.json().await.unwrap();
    assert_eq!(receipt["app_ref"], app_ref);
    assert_eq!(receipt["event"], "ping");
    assert_eq!(receipt["status"], "ignored");
    assert_eq!(receipt["task_ids"], json!([]));
    let receipt_id = receipt["receipt_id"].as_str().unwrap();
    for path in [
        format!("/v1/github/deliveries/{receipt_id}"),
        format!("/v1/github/apps/{app_ref}/deliveries/ping-001"),
    ] {
        let response = fixture.read(&second, &path, "token-alice").await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.json::<serde_json::Value>().await.unwrap(), receipt);
    }
    let metadata = fixture
        .read(
            &second,
            &format!("/v1/github/apps/{app_ref}"),
            "token-alice",
        )
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(metadata["status"], "pending_verification");
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn draft_app_ids_do_not_share_receipts_or_credentials_across_owners() {
    let fixture = Fixture::new(true).await;
    let alice_secret = "synthetic-alice-ping-secret";
    let bob_secret = "synthetic-bob-ping-secret";
    let alice = fixture
        .register("token-alice", "alice-ping", Some(alice_secret))
        .await;
    let bob = fixture
        .register("token-bob", "bob-ping", Some(bob_secret))
        .await;
    let body = r#"{"zen":"Keep it simple","hook_id":9,"hook":{}}"#;
    let mut receipts = Vec::new();
    for (app, secret, token) in [
        (&alice, alice_secret, "token-alice"),
        (&bob, bob_secret, "token-bob"),
    ] {
        let response = fixture
            .webhook(
                app["webhook_url"].as_str().unwrap(),
                secret,
                "same-delivery",
                "ping",
                body,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let receipt: serde_json::Value = response.json().await.unwrap();
        let receipt_id = receipt["receipt_id"].as_str().unwrap();
        assert_eq!(
            fixture
                .read(
                    &fixture.base,
                    &format!("/v1/github/deliveries/{receipt_id}"),
                    token
                )
                .await
                .status(),
            200
        );
        receipts.push(receipt);
    }
    assert_ne!(receipts[0]["receipt_id"], receipts[1]["receipt_id"]);
    assert_eq!(
        fixture
            .read(
                &fixture.base,
                &format!(
                    "/v1/github/deliveries/{}",
                    receipts[0]["receipt_id"].as_str().unwrap()
                ),
                "token-bob"
            )
            .await
            .status(),
        404
    );
    assert_eq!(
        fixture
            .webhook(
                alice["webhook_url"].as_str().unwrap(),
                bob_secret,
                "same-delivery",
                "ping",
                body
            )
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let missing = fixture.register("token-alice", "no-credential", None).await;
    assert_eq!(
        fixture
            .webhook(
                missing["webhook_url"].as_str().unwrap(),
                alice_secret,
                "missing",
                "ping",
                body
            )
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        fixture
            .webhook(
                alice["webhook_url"].as_str().unwrap(),
                alice_secret,
                "invalid-ping",
                "ping",
                "{}"
            )
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        fixture
            .read(
                &fixture.base,
                &format!(
                    "/v1/github/apps/{}/deliveries/invalid-ping",
                    alice["app_ref"].as_str().unwrap()
                ),
                "token-alice"
            )
            .await
            .status(),
        404
    );
}
