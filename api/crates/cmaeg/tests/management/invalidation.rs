use super::*;

#[tokio::test]
async fn update_rekey_and_delete_invalidate_the_local_resolver() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let resolver = state.resolver.clone();
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    let id = initial["channel_id"].as_str().unwrap();
    let alias = cmaeg_core::ids::ChannelId::new(id)
        .unwrap()
        .open(&key_ring(1))
        .unwrap()
        .payload
        .seal_id(&key_ring(1))
        .unwrap();
    let url = format!("{base}/v1/channels/{id}");
    assert_eq!(resolver.resolve(id).await.unwrap().version, 1);
    assert_eq!(resolver.resolve(alias.as_str()).await.unwrap().version, 1);
    let mut update = create_body();
    update["expected_version"] = 1.into();
    update["name"] = "new-name".into();
    assert_eq!(
        client
            .put(&url)
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(resolver.cache_len(), 0);
    assert_eq!(
        resolver.resolve(id).await.unwrap().definition.name,
        "new-name"
    );
    assert_eq!(resolver.resolve(alias.as_str()).await.unwrap().version, 2);
    assert_eq!(
        client
            .post(format!("{url}/rekey"))
            .bearer_auth("cma-service")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(resolver.cache_len(), 0);
    assert_eq!(resolver.resolve(id).await.unwrap().version, 3);
    assert_eq!(resolver.resolve(alias.as_str()).await.unwrap().version, 3);
    assert_eq!(
        client
            .delete(format!("{url}?expected_version=3"))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(resolver.cache_len(), 0);
    assert!(cma.records().is_empty());
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
