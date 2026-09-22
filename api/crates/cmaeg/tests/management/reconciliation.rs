use super::*;

#[tokio::test]
async fn retained_credentials_reconcile_after_failed_rotation_and_replica_restart() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    let channel = initial["channel_id"].as_str().unwrap();
    let mut update = create_body();
    update["expected_version"] = initial["version"].clone();
    update["credentials"] = json!({"agent_key":uuid::Uuid::new_v4().to_string()});
    cma.fail_put
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        client
            .put(format!("{base}/v1/channels/{channel}"))
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    let requests = provider.requests();
    let original: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    let attempted: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_ne!(original["agent_key"], attempted["agent_key"]);
    stop.send(()).unwrap();
    task.await.unwrap();

    cma.fail_put
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let retained: serde_json::Value = client
        .get(format!("{base}/v1/channels/{channel}"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retained["version"], initial["version"]);
    update.as_object_mut().unwrap().remove("credentials");
    assert_eq!(
        client
            .put(format!("{base}/v1/channels/{channel}"))
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].path, requests[0].path);
    let reconciled: serde_json::Value = serde_json::from_str(&requests[2].body).unwrap();
    assert_eq!(reconciled["agent_key"], original["agent_key"]);
    assert_eq!(reconciled["profile_digest"], original["profile_digest"]);
    assert!(requests.iter().all(|request| request.method != "DELETE"));
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
