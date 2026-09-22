use super::*;

#[tokio::test]
async fn create_read_owner_fence_and_delete_round_trip() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, shutdown, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&create_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["channel_id"]
        .as_str()
        .unwrap()
        .starts_with("cmaeg1.ch."));
    assert!(!body.to_string().contains("nyx_agent_secret"));
    assert_eq!(body["version"], 1);
    let stored = cma.records();
    let stored = stored.values().next().unwrap();
    assert_eq!(stored.owner, "alice");
    assert!(!stored.blob.is_empty());
    assert_eq!(stored.version, 1);
    assert!(!String::from_utf8_lossy(&stored.blob).contains("nyx_agent_secret"));
    let opened = cmaeg_core::seal::open_blob(&key_ring(1), &stored.blob).unwrap();
    let private: serde_json::Value = serde_json::from_slice(&opened.payload).unwrap();
    assert_eq!(
        private["secrets"]["provider_credential"],
        "nyx_agent_secret"
    );
    let binding: serde_json::Value = serde_json::from_str(&provider.requests()[0].body).unwrap();
    assert_eq!(binding["profile_digest"], private["profile_digest"]);
    assert_eq!(binding["agent_key"], "nyx_agent_secret");
    assert!(!binding.to_string().contains("nyx_route_secret"));
    let pointer = cmaeg_core::ids::ChannelId::new(body["channel_id"].as_str().unwrap())
        .unwrap()
        .open(&key_ring(1))
        .unwrap()
        .payload;
    assert_eq!(
        pointer.record_url,
        format!(
            "{}/api/v1/gateway/channel-records/{}",
            cma.base_url,
            body["record_id"].as_str().unwrap()
        )
    );
    let records_client = RecordClient::new(&cma.base_url, "cma-service").unwrap();
    assert_eq!(records_client.list(None, None, 100).await.unwrap().len(), 1);
    let channel_id = body["channel_id"].as_str().unwrap().to_owned();

    let read = client
        .get(format!("{base}/v1/channels/{channel_id}"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    let read_body: serde_json::Value = read.json().await.unwrap();
    assert!(!read_body.to_string().contains("hmac-secret"));

    let other = client
        .get(format!("{base}/v1/channels/{channel_id}"))
        .bearer_auth("token-bob")
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 404);

    let deleted = client
        .delete(format!(
            "{base}/v1/channels/{channel_id}?expected_version=1"
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    assert!(cma.records().is_empty());
    assert!(provider
        .requests()
        .iter()
        .any(|request| request.method == "DELETE"));
    shutdown.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn update_preserves_identity_credentials_and_rejects_stale_or_invalid_writes() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    let url = format!(
        "{base}/v1/channels/{}",
        initial["channel_id"].as_str().unwrap()
    );
    let mut update = create_body();
    update.as_object_mut().unwrap().remove("credentials");
    update["expected_version"] = 1.into();
    update["name"] = "renamed".into();
    let response = client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["channel_id"], initial["channel_id"]);
    assert_eq!(
        body["provider"]["binding_id"],
        initial["provider"]["binding_id"]
    );
    assert_eq!(body["definition"]["version"], 2);
    assert_eq!(provider.requests().len(), 1);
    let stale = client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    assert_eq!(stale.headers()["content-type"], "application/problem+json");
    update["expected_version"] = 2.into();
    update["credentials"] = json!({"agent_key":"rotated-agent-fixture"});
    let rotated = client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(rotated.status(), 200);
    let records = cma.records();
    let stored = records.values().next().unwrap();
    let private: serde_json::Value = serde_json::from_slice(
        &cmaeg_core::seal::open_blob(&key_ring(1), &stored.blob)
            .unwrap()
            .payload,
    )
    .unwrap();
    assert_eq!(
        private["secrets"]["provider_credential"],
        "rotated-agent-fixture"
    );
    assert_eq!(private["secrets"]["channel_key"], "nyx_route_secret");
    assert_eq!(
        private["secrets"]["agent_key_hash"],
        cmaeg::channels::definition::hash_secret("rotated-agent-fixture")
    );
    assert_eq!(provider.requests()[0].path, provider.requests()[1].path);
    update["expected_version"] = 3.into();
    update["profile"]["context"]["system_prompt"] = "new instructions".into();
    cma.fail_put
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        client
            .put(&url)
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert!(provider
        .requests()
        .iter()
        .all(|request| request.method != "DELETE"));
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn channel_can_be_created_pending_then_attached_to_a_route() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, shutdown, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let mut request = create_body();
    request["credentials"] = json!({"agent_key":uuid::Uuid::new_v4().to_string(),"channel_key":uuid::Uuid::new_v4().to_string()});
    request["sources"][0]["route_ids"] = json!([]);
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    assert_eq!(created["definition"]["sources"][0]["route_ids"], json!([]));
    let url = format!(
        "{base}/v1/channels/{}",
        created["channel_id"].as_str().unwrap()
    );
    request.as_object_mut().unwrap().remove("credentials");
    request["expected_version"] = created["version"].clone();
    request["sources"][0]["route_ids"] = json!(["new-route"]);
    let response = client
        .put(url)
        .bearer_auth("token-alice")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let updated: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        updated["definition"]["sources"][0]["route_ids"],
        json!(["new-route"])
    );
    assert_eq!(updated["channel_id"], created["channel_id"]);
    shutdown.send(()).unwrap();
    task.await.unwrap();
}
