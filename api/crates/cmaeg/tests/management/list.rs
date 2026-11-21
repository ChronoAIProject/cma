//! Legacy channel records remain visible without resealing their public identity.
use super::*;

#[tokio::test]
async fn listing_mixes_legacy_and_recoverable_channels_without_rewriting_them() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let mut legacy = created(&client, &base).await;
    legacy["provider"]["honoured"] = json!([]);
    legacy["provider"]["refused"] = json!([]);
    let mut body = create_body();
    body["record_id"] = uuid::Uuid::new_v4().to_string().into();
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let mut modern: serde_json::Value = response.json().await.unwrap();
    modern["provider"]["honoured"] = json!([]);
    modern["provider"]["refused"] = json!([]);
    let original_records = cma.records();
    for _ in 0..2 {
        let response = client
            .get(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let page: serde_json::Value = response.json().await.unwrap();
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let old = items
            .iter()
            .find(|item| item["record_id"] == legacy["record_id"])
            .unwrap();
        assert!(old.get("channel_id").is_none());
        assert!(old.get("endpoints").is_none());
        assert_eq!(old["definition"], legacy["definition"]);
        assert_eq!(old["version"], legacy["version"]);
        let new = items
            .iter()
            .find(|item| item["record_id"] == modern["record_id"])
            .unwrap();
        assert_eq!(new, &modern);
    }
    for (id, original) in original_records {
        let retained = cma.records().remove(&id).unwrap();
        assert_eq!(retained.version, original.version);
        assert_eq!(retained.blob, original.blob);
    }
    // Existing integrations continue to use the exact previously issued ID.
    let read = client
        .get(format!(
            "{base}/v1/channels/{}",
            legacy["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    assert_eq!(read.json::<serde_json::Value>().await.unwrap(), legacy);
    let foreign = client
        .get(format!("{base}/v1/channels"))
        .bearer_auth("token-bob")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 200);
    assert!(foreign.json::<serde_json::Value>().await.unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
