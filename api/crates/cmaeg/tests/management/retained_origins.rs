//! A gateway that moves to another CMA address keeps resolving the channel IDs
//! it issued before the move, but only for origins the operator retains.
use super::*;

/// The same gateway after its CMA address moved from `cma.base_url` to an
/// equivalent `localhost` origin of the same record store.
fn moved_state(
    provider: &FakeProvider,
    cma: &FakeCma,
    nyxid: &FakeNyxid,
    retained: &[String],
) -> ManagementState {
    let moved = cma.base_url.replace("127.0.0.1", "localhost");
    assert_ne!(moved, cma.base_url);
    let providers = Arc::new(
        ProviderClient::new(
            registry(provider.base_url()),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            8,
        )
        .unwrap(),
    );
    let records = Arc::new(
        RecordClient::new(&moved, "cma-service")
            .unwrap()
            .with_retained_origins(retained)
            .unwrap(),
    );
    ManagementState::new(
        providers,
        records,
        Arc::new(CreatorAuthenticator::new(&nyxid.base_url).unwrap()),
        cmaeg::ManagementConfig {
            console_public_url: None,
            master_keys: Arc::new(key_ring(1)),
            public_url: "http://127.0.0.1:8090".into(),
            public_api_url: "http://127.0.0.1:8090".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            cma_base_url: moved,
            tools_nyxid_slug: "cmaeg".into(),
            cma_service_token: cmaeg_core::channel::Secret::new("cma-service"),
            nyxid_base_url: nyxid.base_url.clone(),
        },
        cmaeg::ChannelCacheConfig::default(),
    )
}

async fn get_status(client: &Client, base: &str, id: &str) -> (u16, serde_json::Value) {
    let response = client
        .get(format!("{base}/v1/channels/{id}"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or_default())
}

#[tokio::test]
async fn channel_ids_issued_before_a_cma_address_move_resolve_only_when_retained() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let client = Client::new();

    let (before, stop_before, task_before) = app(&provider, &cma, &nyxid).await;
    let channel = created(&client, &before).await;
    let id = channel["channel_id"].as_str().unwrap().to_owned();
    assert_eq!(get_status(&client, &before, &id).await.0, 200);
    stop_before.send(()).unwrap();
    task_before.await.unwrap();

    // Without the earlier origin, the moved gateway cannot tell the sealed
    // record URL from one that names another CMA, and refuses it.
    let (unretained, stop, task) = serve_app(moved_state(&provider, &cma, &nyxid, &[])).await;
    let (status, problem) = get_status(&client, &unretained, &id).await;
    assert_eq!(status, 404);
    assert_eq!(problem["code"], "channel_not_found");
    stop.send(()).unwrap();
    task.await.unwrap();

    // Retaining the earlier origin resolves the same record under the same ID.
    let (moved, stop, task) = serve_app(moved_state(
        &provider,
        &cma,
        &nyxid,
        std::slice::from_ref(&cma.base_url),
    ))
    .await;
    let (status, body) = get_status(&client, &moved, &id).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["channel_id"], id);
    assert_eq!(body["name"], channel["name"]);
    // Channels created after the move are sealed under the configured origin.
    let fresh = created(&client, &moved).await;
    let fresh_id = fresh["channel_id"].as_str().unwrap();
    assert_eq!(get_status(&client, &moved, fresh_id).await.0, 200);
    stop.send(()).unwrap();
    task.await.unwrap();
}
