use super::*;
use cmaeg_core::ids::{ChannelId, ConversationId};

#[tokio::test]
async fn partition_inspection_is_owner_authorized_read_only_and_reports_deleted_agents() {
    let provider = FakeProvider::start(FakeProviderConfig {
        inspection_agent: Some(("agt_deleted".into(), "deleted".into())),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let channel = created(&client, &base).await;
    let id = channel["channel_id"].as_str().unwrap();
    let expected = ConversationId::derive(&key_ring(1), &ChannelId::new(id).unwrap(), 1, "oracle");
    let url = format!("{base}/v1/channels/{id}/partition?partition_key=oracle");
    let before = provider.log().len();
    for (token, status) in [("token-bob", 404), ("invalid", 401)] {
        let response = client.get(&url).bearer_auth(token).send().await.unwrap();
        assert_eq!(response.status(), status);
    }
    assert_eq!(provider.log().len(), before);
    let response = client
        .get(&url)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["transport_conversation_id"], expected.as_str());
    assert_eq!(body["mapping"]["conversation_id"], expected.as_str());
    assert_eq!(body["mapping"]["agent_id"], "agt_deleted");
    assert_eq!(body["mapping"]["agent_status"], "deleted");
    assert_eq!(
        body["mapping"]["binding_id"],
        channel["provider"]["binding_id"]
    );
    assert_eq!(provider.log().len(), before + 1);
    assert_eq!(provider.log().last().unwrap().method, "GET");
    assert!(!body.to_string().contains("nyx_agent_secret"));
    let missing = client
        .get(format!("{base}/v1/channels/{id}/partition"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 400);
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn absent_partition_does_not_create_a_provider_conversation() {
    let provider = FakeProvider::start(Default::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let channel = created(&client, &base).await;
    let before = provider.log().len();
    let response = client
        .get(format!(
            "{base}/v1/channels/{}/partition?partition_key=unused",
            channel["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(provider.log().len(), before + 1);
    assert_eq!(provider.log().last().unwrap().method, "GET");
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn incomplete_existing_agent_creation_has_an_actionable_refusal_and_exact_retry() {
    let provider = FakeProvider::start(FakeProviderConfig {
        binding_failures: [(0, (409, json!({"code":"binding_not_ready"})))].into(),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let resolver = state.resolver.clone();
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let mut request = create_body();
    request["record_id"] = uuid::Uuid::new_v4().to_string().into();
    request["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_retained"});
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let problem: serde_json::Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "provider_rejected");
    let saved: serde_json::Value = client
        .get(format!(
            "{base}/v1/channel-records/{}",
            request["record_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = saved["channel_id"].as_str().unwrap();
    assert_eq!(
        resolver.resolve(id).await.unwrap_err(),
        cmaeg::channels::resolver::ResolveError::CreationPending
    );
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let completed: serde_json::Value = response.json().await.unwrap();
    assert_eq!(completed["channel_id"], id);
    assert_eq!(completed["definition"]["creation"]["ready"], true);
    resolver.resolve(id).await.unwrap();
    assert_eq!(cma.records().len(), 1);
    stop.send(()).unwrap();
    task.await.unwrap();
}
