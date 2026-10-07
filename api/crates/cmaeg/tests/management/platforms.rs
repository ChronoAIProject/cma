use super::*;
use cmaeg::channels::definition::{
    build_definition, build_update, validate_create_request, validate_update_request,
    ChannelCreateRequest, ChannelUpdateRequest,
};
use cmaeg_core::{
    channel::{ConformanceLevel, Platform, Source},
    ids::{BindingId, RecordId},
};

fn build(body: serde_json::Value) -> Result<Source, cmaeg_core::ValidationError> {
    let request: ChannelCreateRequest = serde_json::from_value(body).unwrap();
    build_definition(
        &request,
        "alice",
        RecordId::new("record").unwrap(),
        BindingId::new("binding").unwrap(),
        ConformanceLevel::L1,
    )
    .map(|definition| definition.sources[0].clone())
}

#[test]
fn legacy_relay_requests_serialize_without_new_fields_so_fingerprints_are_stable() {
    let request: ChannelCreateRequest = serde_json::from_value(create_body()).unwrap();
    let source = serde_json::to_value(&request.sources[0]).unwrap();
    assert!(source.get("bot_id").is_none(), "{source}");
    assert!(source.get("trust_normalized").is_none(), "{source}");
}

#[test]
fn relay_platform_identity_and_trust_are_carried_into_the_definition() {
    let mut body = create_body();
    body["sources"][0]["platform"] = "lark".into();
    body["sources"][0]["bot_id"] = "ou_bot".into();
    let Source::NyxidRelay {
        platform,
        bot_id,
        trust_normalized,
        ..
    } = build(body.clone()).unwrap()
    else {
        panic!("expected a relay source");
    };
    assert_eq!(
        (platform, bot_id.as_deref(), trust_normalized),
        (Platform::Lark, Some("ou_bot"), false)
    );
    body["sources"][0]["bot_id"] = "cli_app".into();
    assert_eq!(
        build(body.clone()).unwrap_err().pointer,
        "/sources/0/bot_id"
    );
    body["sources"][0]["bot_id"] = "ou_bot".into();
    body["sources"][0]["trust_normalized"] = true.into();
    assert_eq!(
        build(body.clone()).unwrap_err().pointer,
        "/sources/0/trust_normalized"
    );
    body["sources"][0]["platform"] = "whatsapp".into();
    body["sources"][0]["bot_id"] = "1234567890".into();
    let Source::NyxidRelay {
        platform,
        trust_normalized,
        ..
    } = build(body).unwrap()
    else {
        panic!("expected a relay source");
    };
    assert_eq!((platform, trust_normalized), (Platform::Whatsapp, true));
}

fn relay_body(platform: &str, trust: Option<bool>) -> serde_json::Value {
    let mut body = create_body();
    body["sources"][0]["platform"] = platform.into();
    if let Some(trust) = trust {
        body["sources"][0]["trust_normalized"] = trust.into();
    }
    body
}

#[test]
fn unverified_platforms_are_refused_on_create_and_update_without_the_opt_in() {
    let current = {
        let request: ChannelCreateRequest =
            serde_json::from_value(relay_body("slack", Some(true))).unwrap();
        build_definition(
            &request,
            "alice",
            RecordId::new("record").unwrap(),
            BindingId::new("binding").unwrap(),
            ConformanceLevel::L1,
        )
        .unwrap()
    };
    for platform in ["discord", "slack", "whatsapp"] {
        for trust in [None, Some(false)] {
            let body = relay_body(platform, trust);
            let create: ChannelCreateRequest = serde_json::from_value(body.clone()).unwrap();
            let mut update = body;
            update.as_object_mut().unwrap().remove("credentials");
            update["expected_version"] = 1.into();
            let update: ChannelUpdateRequest = serde_json::from_value(update).unwrap();
            for error in [
                validate_create_request(&create).unwrap_err(),
                build(relay_body(platform, trust)).unwrap_err(),
                validate_update_request(&update).unwrap_err(),
                build_update(&update, &current).unwrap_err(),
            ] {
                assert_eq!(error.pointer, "/sources/0/trust_normalized", "{platform}");
                assert!(error.message.contains("no raw-event verifier"), "{error}");
            }
        }
        build(relay_body(platform, Some(true))).unwrap();
    }
    for platform in ["telegram", "lark", "feishu"] {
        let body = relay_body(platform, None);
        validate_create_request(&serde_json::from_value(body.clone()).unwrap()).unwrap();
        build(body).unwrap();
    }
}

#[tokio::test]
async fn http_create_and_update_fail_fast_for_unverified_platforms_and_unknown_spellings() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let url = format!("{base}/v1/channels");
    for (body, detail) in [
        (
            relay_body("discord", None),
            "/sources/0/trust_normalized: the platform has no raw-event verifier",
        ),
        (
            relay_body("whatsapp", Some(false)),
            "/sources/0/trust_normalized: the platform has no raw-event verifier",
        ),
        (
            relay_body("x", Some(true)),
            "request does not match the channel schema",
        ),
        (
            relay_body("aurinko", None),
            "request does not match the channel schema",
        ),
    ] {
        let response = client
            .post(&url)
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        let problem: serde_json::Value = response.json().await.unwrap();
        assert_eq!(problem["code"], "invalid_request");
        assert!(
            problem["detail"].as_str().unwrap().starts_with(detail),
            "{problem}"
        );
    }
    assert!(provider.requests().is_empty());
    assert!(cma.records().is_empty());
    let response = client
        .post(&url)
        .bearer_auth("token-alice")
        .json(&relay_body("slack", Some(true)))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        created["definition"]["sources"][0]["trust_normalized"],
        true
    );
    let before = cma.records();
    let calls = provider.requests().len();
    let mut update = relay_body("slack", None);
    update.as_object_mut().unwrap().remove("credentials");
    update["expected_version"] = created["version"].clone();
    let response = client
        .put(format!("{url}/{}", created["channel_id"].as_str().unwrap()))
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let problem: serde_json::Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(provider.requests().len(), calls);
    let after = cma.records();
    for (record_id, record) in &before {
        assert_eq!(after[record_id].version, record.version);
        assert_eq!(after[record_id].blob, record.blob);
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn native_source_exposes_webhook_endpoint_and_never_returns_platform_credentials() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let mut body = create_body();
    body["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
    body["credentials"]["telegram_bot_token"] = "77:fixture".into();
    body["credentials"]["telegram_webhook_secret"] = "webhook-fixture".into();
    let response = Client::new()
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let value: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        value["endpoints"]["telegram_webhook_url"],
        format!(
            "http://127.0.0.1:8090/v1/telegram/{}",
            value["channel_id"].as_str().unwrap()
        )
    );
    let public = value.to_string();
    assert!(!public.contains("77:fixture"));
    assert!(!public.contains("webhook-fixture"));
    assert!(value["definition"]["secrets"]
        .get("telegram_bot_token")
        .is_none());
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn thread_contract_is_advertised_then_echoed_only_for_supported_sources() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let mut body = relay_body("lark", None);
    body["sources"][0]["bot_id"] = json!("ou_bot");
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    assert_eq!(created["thread_contract_versions"], json!([1]));
    assert!(created["definition"]["reply"]
        .get("thread_contract")
        .is_none());
    body.as_object_mut().unwrap().remove("credentials");
    body["expected_version"] = created["version"].clone();
    body["reply"]["thread_contract"] = json!({"version":1,"follow_chat_ids":["oc_chat"]});
    let response = client
        .put(format!(
            "{base}/v1/channels/{}",
            created["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let updated: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        updated["definition"]["reply"]["thread_contract"],
        body["reply"]["thread_contract"]
    );
    stop.send(()).ok();
    task.await.unwrap();
}
