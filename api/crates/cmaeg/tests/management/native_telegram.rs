//! Owner authoring supplies durable selection without operator record lists.
use super::*;
use serde_json::Value;
use std::time::Duration;

fn body(opted: bool) -> Value {
    let mut value = create_body();
    value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
    if opted {
        value["sources"][0]["message_events"] = true.into();
    }
    value["credentials"]["telegram_bot_token"] = "77:fixture-token".into();
    value["credentials"]["telegram_webhook_secret"] = "fixture-webhook".into();
    value
}
fn state(provider: &FakeProvider, cma: &FakeCma, nyxid: &FakeNyxid) -> ManagementState {
    let mut state = management_state(provider, cma, nyxid, key_ring(1));
    let registry = ProviderRegistry::parse(&json!([{
        "slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http",
        "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
        "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses",
        "recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}},"max_inflight":8
    }]).to_string()).unwrap();
    state.providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    );
    state.native_telegram_message_events_writes_enabled = true;
    state.native_telegram_durable_admission_enabled = true;
    state.multi_consumer_writes_enabled = true;
    state
}
async fn post(base: &str, body: &Value) -> reqwest::Response {
    Client::new()
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(body)
        .send()
        .await
        .unwrap()
}
#[tokio::test]
async fn telegram_message_events_owner_create_and_update_preserve_default_binding_and_legacy_shape()
{
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = serve_app(state(&provider, &cma, &nyxid)).await;
    for reserved in [false, true] {
        let mut input = body(true);
        if reserved {
            input["record_id"] = uuid::Uuid::new_v4().to_string().into();
        }
        let response = post(&base, &input).await;
        let status = response.status();
        let saved: Value = response.json().await.unwrap();
        assert_eq!(status, 201, "{saved}");
        assert_eq!(saved["definition"]["sources"][0]["message_events"], true);
        assert_eq!(
            saved["definition"]["routing"]["default_targets"],
            json!(["default"])
        );
        assert!(saved["definition"]["routing"].get("consumers").is_none());
    }
    let input = body(false);
    let saved: Value = post(&base, &input).await.json().await.unwrap();
    assert!(saved["definition"].get("routing").is_none());
    assert!(saved["definition"]["sources"][0]
        .get("message_events")
        .is_none());
    let original_provider = saved["definition"]["provider"].clone();
    let mut update = input;
    update.as_object_mut().unwrap().remove("credentials");
    update["sources"][0]["message_events"] = true.into();
    update["expected_version"] = saved["version"].clone();
    let url = format!(
        "{base}/v1/channels/{}",
        saved["channel_id"].as_str().unwrap()
    );
    let response = Client::new()
        .put(&url)
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let saved: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["definition"]["provider"], original_provider);
    assert_eq!(
        saved["definition"]["routing"]["default_targets"],
        json!(["default"])
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn telegram_message_events_owner_opt_in_requires_gates_and_provider_recovery_before_effects()
{
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    for case in 0..4 {
        let mut state = state(&provider, &cma, &nyxid);
        match case {
            0 => state.native_telegram_message_events_writes_enabled = false,
            1 => state.native_telegram_durable_admission_enabled = false,
            2 => state.multi_consumer_writes_enabled = false,
            _ => state.providers = management_state(&provider, &cma, &nyxid, key_ring(1)).providers,
        }
        let (base, stop, task) = serve_app(state).await;
        assert_eq!(post(&base, &body(true)).await.status(), 400, "case {case}");
        assert!(provider.requests().is_empty());
        assert!(cma.records().is_empty());
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn telegram_topic_owner_enrollment_is_automatic_and_independent_of_message_events() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut settings = state(&provider, &cma, &nyxid);
    settings.native_telegram_message_events_writes_enabled = false;
    settings.native_telegram_topic_lifecycle_enabled = true;
    settings.native_telegram_topic_creation_enabled = true;
    let (base, stop, task) = serve_app(settings).await;
    for consent in ["topic_lifecycle", "topic_creation"] {
        let input = body(false);
        let saved: Value = post(&base, &input).await.json().await.unwrap();
        assert!(saved["definition"].get("routing").is_none());
        let provider = saved["definition"]["provider"].clone();
        let mut update = input;
        update.as_object_mut().unwrap().remove("credentials");
        update["sources"][0][consent] = true.into();
        update["expected_version"] = saved["version"].clone();
        let url = format!(
            "{base}/v1/channels/{}",
            saved["channel_id"].as_str().unwrap()
        );
        let response = Client::new()
            .put(&url)
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let saved: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{saved}");
        assert_eq!(saved["definition"]["provider"], provider);
        assert_eq!(
            saved["definition"]["routing"]["default_targets"],
            json!(["default"])
        );
        assert_eq!(
            saved["definition"]["routing"]["default_consumer"]["name"],
            "default"
        );
        assert!(saved["definition"]["sources"][0]
            .get("message_events")
            .is_none());
        assert_eq!(saved["definition"]["sources"][0][consent], true);
        update["sources"][0][consent] = false.into();
        update["expected_version"] = saved["version"].clone();
        let response = Client::new()
            .put(&url)
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let opted_out: Value = response.json().await.unwrap();
        assert_eq!(
            opted_out["definition"]["routing"],
            saved["definition"]["routing"]
        );
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn telegram_topic_owner_opt_in_requires_own_gate_and_durable_recovery() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    for consent in ["topic_lifecycle", "topic_creation"] {
        for case in 0..4 {
            let mut settings = state(&provider, &cma, &nyxid);
            settings.native_telegram_topic_lifecycle_enabled = consent == "topic_lifecycle";
            settings.native_telegram_topic_creation_enabled = consent == "topic_creation";
            match case {
                0 => {
                    settings.native_telegram_topic_lifecycle_enabled = false;
                    settings.native_telegram_topic_creation_enabled = false;
                }
                1 => settings.native_telegram_durable_admission_enabled = false,
                2 => settings.multi_consumer_writes_enabled = false,
                _ => {
                    settings.providers =
                        management_state(&provider, &cma, &nyxid, key_ring(1)).providers
                }
            }
            let (base, stop, task) = serve_app(settings).await;
            let mut input = body(false);
            input["sources"][0][consent] = true.into();
            assert_eq!(post(&base, &input).await.status(), 400, "case {case}");
            assert!(provider.requests().is_empty());
            assert!(cma.records().is_empty());
            stop.send(()).unwrap();
            task.await.unwrap();
        }
    }
}
