//! Self-service native authoring is explicit, recoverable and secret-safe.
use super::*;
use cmaeg::{discord::selection::Selection, NativeDiscordConfig};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};

fn body() -> Value {
    let mut value = create_body();
    value["record_id"] = uuid::Uuid::new_v4().to_string().into();
    value["sources"] =
        json!([{"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"}}]);
    value["credentials"]["discord_bot_token"] = "native-owner-private-token".into();
    value
}
fn native_state(provider: &FakeProvider, cma: &FakeCma, nyxid: &FakeNyxid) -> ManagementState {
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
    state.native_discord_writes_enabled = true;
    state.multi_consumer_writes_enabled = true;
    state.native_discord_admission_enabled = true;
    state.native_discord = Selection::new(&NativeDiscordConfig {
        enabled: true,
        discovery_enabled: true,
        rich_content_enabled: false,
        records: BTreeSet::new(),
    });
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
async fn native_creation_seeds_only_default_routing_and_retries_preserve_identity_and_secrets() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let state = native_state(&provider, &cma, &nyxid);
    let (base, stop, task) = serve_app(state.clone()).await;
    let input = body();
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(created["definition"]["creation"]["ready"], true);
    assert_eq!(
        created["definition"]["routing"]["default_consumer"]["name"],
        "default"
    );
    assert_eq!(
        created["definition"]["routing"]["default_targets"],
        json!(["default"])
    );
    assert!(created["definition"]["routing"].get("consumers").is_none());
    assert_eq!(
        created["native_discord_activation"]["state"],
        "pending_discovery"
    );
    assert!(!created.to_string().contains("native-owner-private-token"));
    let binds = provider.requests().len();
    stop.send(()).unwrap();
    task.await.unwrap();
    let mut restarted = state;
    restarted.native_discord_writes_enabled = false;
    restarted.multi_consumer_writes_enabled = false;
    let (base, stop, task) = serve_app(restarted).await;
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let retried: Value = response.json().await.unwrap();
    for field in [
        "channel_id",
        "record_id",
        "version",
        "definition",
        "endpoints",
        "native_discord_activation",
    ] {
        assert_eq!(retried[field], created[field], "{field}");
    }
    assert_eq!(
        retried["provider"]["binding_id"],
        created["provider"]["binding_id"]
    );
    assert_eq!(provider.requests().len(), binds);
    assert_eq!(cma.records().len(), 1);
    let saved = &cma.records()[input["record_id"].as_str().unwrap()];
    let opened = cmaeg_core::seal::open_blob(&key_ring(1), &saved.blob).unwrap();
    let saved: Value = serde_json::from_slice(&opened.payload).unwrap();
    assert_eq!(
        saved["secrets"]["discord_bot_token"],
        "native-owner-private-token"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn native_creation_refuses_missing_stable_id_gates_and_nonrecoverable_provider_before_effects(
) {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    for case in 0..5 {
        let mut state = native_state(&provider, &cma, &nyxid);
        let mut input = body();
        match case {
            0 => {
                input.as_object_mut().unwrap().remove("record_id");
            }
            1 => input["record_id"] = "00000000-0000-1000-8000-000000000001".into(),
            2 => state.native_discord_writes_enabled = false,
            3 => state.multi_consumer_writes_enabled = false,
            _ => state.providers = management_state(&provider, &cma, &nyxid, key_ring(1)).providers,
        }
        let (base, stop, task) = serve_app(state).await;
        assert_eq!(post(&base, &input).await.status(), 400, "case {case}");
        assert!(cma.records().is_empty());
        assert!(provider.requests().is_empty());
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}
#[tokio::test]
async fn canary_mode_and_existing_relay_creation_do_not_gain_implicit_routing() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    for native in [false, true] {
        let mut state = native_state(&provider, &cma, &nyxid);
        let input = if native {
            state.native_discord = Selection::new(&NativeDiscordConfig {
                enabled: true,
                ..Default::default()
            });
            body()
        } else {
            create_body()
        };
        let (base, stop, task) = serve_app(state).await;
        let response = post(&base, &input).await;
        assert_eq!(response.status(), 201);
        let saved: Value = response.json().await.unwrap();
        assert!(saved["definition"].get("routing").is_none());
        if native {
            assert_eq!(
                saved["native_discord_activation"]["state"],
                "routing_required"
            );
        } else {
            assert!(saved.get("native_discord_activation").is_none());
        }
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}
#[tokio::test]
async fn full_native_capacity_is_visible_in_saved_channel_and_source_check() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut state = native_state(&provider, &cma, &nyxid);
    state.native_discord = Selection::new(&NativeDiscordConfig {
        enabled: true,
        discovery_enabled: true,
        rich_content_enabled: false,
        records: (0..64).map(|_| uuid::Uuid::new_v4().to_string()).collect(),
    });
    let (base, stop, task) = serve_app(state).await;
    let saved: Value = post(&base, &body()).await.json().await.unwrap();
    assert_eq!(
        saved["native_discord_activation"]["state"],
        "capacity_exceeded"
    );
    let check: Value = Client::new()
        .post(format!(
            "{base}/v1/channels/{}/sources/0/test",
            saved["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let activation = check["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "native_activation")
        .unwrap();
    assert_eq!(activation["status"], "unavailable");
    assert!(activation["detail"]
        .as_str()
        .unwrap()
        .contains("capacity is full"));
    assert!(!check.to_string().contains("native-owner-private-token"));
    stop.send(()).unwrap();
    task.await.unwrap();
}
