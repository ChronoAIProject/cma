use super::*;

#[tokio::test]
async fn disabled_github_rejects_source_creation_and_updates_without_changing_channels() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
    state.dynamic_sources_writes_enabled = true;
    let (base, stop, server) = serve_app(state).await;
    let client = Client::new();
    let existing = created(&client, &base).await;
    let channel_id = existing["channel_id"].as_str().unwrap();
    let mut body = create_body();
    body["partition"]["custom_key"] = "per_event".into();
    body["sources"].as_array_mut().unwrap().push(json!({
        "type":"github", "app_ref":"pilot", "installation_ids":[7],
        "repository_ids":[42], "events":["issues"], "actions":["opened"]
    }));
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["detail"],
        "/sources: github_writes_disabled"
    );
    body.as_object_mut().unwrap().remove("credentials");
    body["expected_version"] = existing["version"].clone();
    let response = client
        .put(format!("{base}/v1/channels/{channel_id}"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["detail"],
        "/sources: github_writes_disabled"
    );
    let response = client
        .get(format!("{base}/v1/channels/{channel_id}"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let retained: serde_json::Value = response.json().await.unwrap();
    assert_eq!(retained["version"], existing["version"]);
    assert_eq!(
        retained["definition"]["sources"].as_array().unwrap().len(),
        1
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
