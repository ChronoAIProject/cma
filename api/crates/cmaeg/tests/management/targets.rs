use super::*;
use cmaeg::channels::definition::{
    build_definition, build_update, validate_create_request, ChannelCreateRequest,
    ChannelUpdateRequest,
};
use cmaeg_core::{
    channel::{ConformanceLevel, ProviderTarget},
    ids::{BindingId, RecordId},
};

#[test]
fn omitted_target_keeps_legacy_profile_contract_and_targets_cannot_change() {
    let mut body = create_body();
    let request: ChannelCreateRequest = serde_json::from_value(body.clone()).unwrap();
    assert!(request.provider.target.is_profile());
    assert!(serde_json::to_value(&request.provider)
        .unwrap()
        .get("target")
        .is_none());
    let original = build_definition(
        &request,
        "alice",
        RecordId::new("record").unwrap(),
        BindingId::new("binding").unwrap(),
        ConformanceLevel::L1,
    )
    .unwrap();
    assert!(original.to_public_json()["provider"]
        .get("target")
        .is_none());
    body["expected_version"] = 1.into();
    body["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_existing"});
    let update: ChannelUpdateRequest = serde_json::from_value(body).unwrap();
    assert_eq!(
        build_update(&update, &original).unwrap_err().pointer,
        "/provider/target"
    );
}

#[test]
fn existing_agent_target_rejects_invalid_identifiers_and_unknown_fields() {
    for id in ["", "../agent", "agent?x=1", "agent with spaces"] {
        let mut body = create_body();
        body["provider"]["target"] = json!({"type":"existing_agent","agent_id":id});
        let request: ChannelCreateRequest = serde_json::from_value(body).unwrap();
        assert!(validate_create_request(&request).is_err());
    }
    for target in [
        json!({"type":"profile","unexpected":true}),
        json!({"type":"existing_agent","agent_id":"agt_existing","owner":"other"}),
    ] {
        assert!(serde_json::from_value::<ProviderTarget>(target).is_err());
    }
}

#[tokio::test]
async fn existing_agent_target_is_retained_and_forwarded_to_provider_unchanged() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, shutdown, task) = app(&provider, &cma, &nyxid).await;
    let mut body = create_body();
    let target = json!({"type":"existing_agent","agent_id":"agt_existing"});
    body["provider"]["target"] = target.clone();
    let response = Client::new()
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let response: serde_json::Value = response.json().await.unwrap();
    assert_eq!(response["definition"]["provider"]["target"], target);
    let requests = provider.requests();
    let bind = requests
        .iter()
        .find(|request| request.method == "PUT" && !request.path.contains("conversations"))
        .unwrap();
    let forwarded: serde_json::Value = serde_json::from_str(&bind.body).unwrap();
    assert_eq!(forwarded["target"], target);
    shutdown.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
}
