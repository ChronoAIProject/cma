use super::*;

#[tokio::test]
async fn creation_failures_validate_before_binding_and_roll_back_store_failure() {
    for (reject, fail_store, expected) in [(true, false, 502), (false, true, 503)] {
        let provider = FakeProvider::start(FakeProviderConfig {
            reject_binding: reject,
            ..Default::default()
        })
        .await;
        let cma = FakeCma::start(fail_store).await;
        let nyxid = FakeNyxid::start().await;
        let (base, stop, task) = app(&provider, &cma, &nyxid).await;
        let client = Client::new();
        let mut invalid = create_body();
        invalid["partition"]["version"] = 99.into();
        assert_eq!(
            client
                .post(format!("{base}/v1/channels"))
                .bearer_auth("token-alice")
                .json(&invalid)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        assert!(provider.requests().is_empty());
        let response = client
            .post(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&create_body())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(cma.records().is_empty());
        assert_eq!(
            provider
                .requests()
                .iter()
                .any(|request| request.method == "DELETE"),
            fail_store
        );
        stop.send(()).unwrap();
        task.await.unwrap();
        provider.close().await;
        cma.close().await;
        nyxid.close().await;
    }
}

#[tokio::test]
async fn concurrent_idempotent_creates_replay_and_different_payload_conflicts() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let send = || {
        client
            .post(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "same-create")
            .json(&create_body())
            .send()
    };
    let (first, second) = tokio::join!(send(), send());
    let first: serde_json::Value = first
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: serde_json::Value = second
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(cma.records().len(), 1);
    assert_eq!(provider.requests().len(), 1);
    let mut different = create_body();
    different["credentials"]["agent_key"] = "different-fixture".into();
    assert_eq!(
        client
            .post(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "same-create")
            .json(&different)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn test_turn_creates_conversation_and_bounds_unicode_text() {
    let provider = FakeProvider::start(FakeProviderConfig {
        text: "你好".repeat(200),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    let before = provider.requests().len();
    let invalid = client
        .post(format!("{base}/v1/channels/invalid/test"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    assert_eq!(invalid.headers()["cache-control"], "no-store");
    assert_eq!(
        invalid.headers()["content-type"],
        "application/problem+json"
    );
    let problem: serde_json::Value = invalid.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(provider.requests().len(), before);
    let response = client
        .post(format!(
            "{base}/v1/channels/{}/test",
            initial["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["accepted_after_ms"].is_number());
    assert_eq!(body["first_text"].as_str().unwrap().chars().count(), 200);
    let requests = provider.requests();
    assert!(requests[1].path.contains("/conversations/conv_"));
    assert!(requests[2]
        .idempotency_key
        .as_ref()
        .unwrap()
        .starts_with("test_"));
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn rekey_requires_service_bearer_and_reseals_old_blob() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    stop.send(()).unwrap();
    task.await.unwrap();
    let (base, stop, task) = app_with_keys(&provider, &cma, &nyxid, key_ring(2)).await;
    let url = format!(
        "{base}/v1/channels/{}/rekey",
        initial["channel_id"].as_str().unwrap()
    );
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = client
        .post(&url)
        .bearer_auth("cma-service")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["kid"], 2);
    let records = cma.records();
    let record = records.values().next().unwrap();
    assert_eq!(record.version, 2);
    assert_eq!(
        cmaeg_core::seal::open_blob(&key_ring(2), &record.blob)
            .unwrap()
            .kid,
        2
    );
    assert_eq!(provider.requests().len(), 1);
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
