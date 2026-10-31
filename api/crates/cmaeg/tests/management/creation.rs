use super::*;
use serde_json::Value;

fn reserved_body() -> Value {
    let mut body = create_body();
    body["record_id"] = uuid::Uuid::new_v4().to_string().into();
    body["credentials"] = json!({"agent_key":uuid::Uuid::new_v4().to_string(),"channel_key":uuid::Uuid::new_v4().to_string()});
    body["sources"][0]["route_ids"] = json!([]);
    body
}

#[tokio::test]
async fn reserved_creation_replays_across_replicas_and_restart_without_rebinding() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let client = Client::new();
    let body = reserved_body();
    let record_id = body["record_id"].as_str().unwrap();
    let (first, stop, task) = app(&provider, &cma, &nyxid).await;
    let response = client
        .post(format!("{first}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(created["record_id"], record_id);
    assert_eq!(created["definition"]["creation"]["ready"], true);
    let binds = provider.requests().len();
    stop.send(()).unwrap();
    task.await.unwrap();
    let (second, stop, task) = app(&provider, &cma, &nyxid).await;
    for (method, url, input) in [
        (
            reqwest::Method::GET,
            format!("{second}/v1/channel-records/{record_id}"),
            None,
        ),
        (
            reqwest::Method::POST,
            format!("{second}/v1/channels"),
            Some(&body),
        ),
    ] {
        let mut request = client.request(method, url).bearer_auth("token-alice");
        if let Some(body) = input {
            request = request.json(body);
        }
        let response = request.send().await.unwrap();
        assert!(response.status().is_success());
        let recovered: Value = response.json().await.unwrap();
        assert_eq!(recovered["channel_id"], created["channel_id"]);
        assert_eq!(recovered["endpoints"], created["endpoints"]);
        assert_eq!(
            recovered["provider"]["binding_id"],
            created["provider"]["binding_id"]
        );
    }
    assert_eq!(provider.requests().len(), binds);
    assert_eq!(cma.records().len(), 1);
    let url = format!("{second}/v1/channel-records/{record_id}");
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("token-bob")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let mut changed = body.clone();
    changed["name"] = "changed".into();
    assert_eq!(
        client
            .post(format!("{second}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&changed)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn failed_binding_retains_encrypted_cleanup_identity_and_blocks_events() {
    let provider = FakeProvider::start(FakeProviderConfig {
        reject_binding: true,
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let resolver = state.resolver.clone();
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let body = reserved_body();
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let recovered: Value = client
        .get(format!(
            "{base}/v1/channel-records/{}",
            body["record_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["definition"]["creation"]["ready"], false);
    let id = cmaeg_core::ids::ChannelId::new(recovered["channel_id"].as_str().unwrap()).unwrap();
    let refused = resolver.resolve(id.as_str()).await.unwrap_err();
    assert_eq!(
        refused,
        cmaeg::channels::resolver::ResolveError::CreationPending
    );
    assert_eq!(refused.problem().status, 409);
    assert!(refused
        .problem()
        .detail
        .unwrap()
        .contains("channel_creation_pending"));
    assert_eq!(cma.records().len(), 1);
    let url = format!(
        "{base}/v1/channels/{id}?expected_version={}",
        recovered["version"]
    );
    assert_eq!(
        client
            .delete(url)
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert!(cma.records().is_empty());
    assert_eq!(provider.requests().last().unwrap().method, "DELETE");
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn concurrent_reserved_creates_have_one_record_and_binding_identity() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (a, stop_a, task_a) = app(&provider, &cma, &nyxid).await;
    let (b, stop_b, task_b) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let body = reserved_body();
    let (a, b) = tokio::join!(
        client
            .post(format!("{a}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&body)
            .send(),
        client
            .post(format!("{b}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.status(), 201);
    assert_eq!(b.status(), 201);
    let a: Value = a.json().await.unwrap();
    let b: Value = b.json().await.unwrap();
    assert_eq!(a["channel_id"], b["channel_id"]);
    assert_eq!(a["provider"]["binding_id"], b["provider"]["binding_id"]);
    assert_eq!(cma.records().len(), 1);
    stop_a.send(()).unwrap();
    stop_b.send(()).unwrap();
    task_a.await.unwrap();
    task_b.await.unwrap();
}
