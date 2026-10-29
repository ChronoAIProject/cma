//! Durable Custom authoring is explicit, immutable, reader-first and recoverable.
use super::*;
use serde_json::Value;
use std::time::Duration;

fn body(opted: bool) -> Value {
    let mut body = create_body();
    body["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"h1"}}]);
    body["partition"] = json!({"version":1,"key":"per_event"});
    body["reply"]["sinks"] = json!([{"type":"sync"}]);
    if opted {
        body["durable_custom_execution"] = true.into();
    }
    body
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
    state.custom_durable_execution_enabled = true;
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
async fn custom_create_stores_mode_and_routing_atomically_for_profile_and_existing_agent() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let configured = state(&provider, &cma, &nyxid);
    let retained_state = configured.clone();
    let (base, stop, task) = serve_app(configured).await;
    for existing in [false, true] {
        let mut input = body(true);
        input["record_id"] = uuid::Uuid::new_v4().to_string().into();
        if existing {
            input["provider"]["target"] =
                json!({"type":"existing_agent","agent_id":"agt_custom_fixture"});
        }
        let response = post(&base, &input).await;
        let status = response.status();
        let saved: Value = response.json().await.unwrap();
        assert_eq!(status, 201, "{saved}");
        assert_eq!(saved["definition"]["durable_custom_execution"], true);
        assert_eq!(
            saved["definition"]["routing"]["default_consumer"]["non_human"],
            true
        );
        assert_eq!(
            saved["definition"]["routing"]["default_targets"],
            json!(["default"])
        );
        assert!(saved["definition"]["routing"].get("consumers").is_none());
        assert_eq!(saved["definition"]["creation"]["ready"], true);
        let binds = provider.requests().len();
        let replay: Value = post(&base, &input).await.json().await.unwrap();
        assert_eq!(replay["channel_id"], saved["channel_id"]);
        assert_eq!(provider.requests().len(), binds);
        let mut changed = input.clone();
        changed["durable_custom_execution"] = false.into();
        assert_eq!(post(&base, &changed).await.status(), 409);

        // An old client's full PUT retains the immutable mode, routing and binding
        // even when the new-write gate is subsequently disabled.
        let mut disabled = retained_state.clone();
        disabled.custom_durable_execution_enabled = false;
        disabled.multi_consumer_writes_enabled = false;
        let (old_client_base, stop_old, task_old) = serve_app(disabled).await;
        let mut update = input;
        for key in ["record_id", "credentials", "durable_custom_execution"] {
            update.as_object_mut().unwrap().remove(key);
        }
        update["name"] = "renamed without changing execution".into();
        update["expected_version"] = saved["version"].clone();
        let response = Client::new()
            .put(format!(
                "{old_client_base}/v1/channels/{}",
                saved["channel_id"].as_str().unwrap()
            ))
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let updated: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{updated}");
        assert_eq!(updated["definition"]["durable_custom_execution"], true);
        assert_eq!(
            updated["definition"]["routing"],
            saved["definition"]["routing"]
        );
        assert_eq!(
            updated["definition"]["provider"],
            saved["definition"]["provider"]
        );
        // Exact previous web channelDraft algorithm: unknown root properties are
        // copied, while known server metadata and provider binding fields are removed.
        let mut update = updated["definition"].clone();
        for key in [
            "record_id",
            "creation",
            "owner",
            "profile_digest",
            "secrets",
            "created_at",
            "version",
            "routing",
        ] {
            update.as_object_mut().unwrap().remove(key);
        }
        for key in ["binding_id", "level"] {
            update["provider"].as_object_mut().unwrap().remove(key);
        }
        for source in update["sources"].as_array_mut().unwrap() {
            source.as_object_mut().unwrap().remove("jwks_url");
        }
        update["expected_version"] = updated["version"].clone();
        assert_eq!(update["durable_custom_execution"], true);
        let echoed = Client::new()
            .put(format!(
                "{old_client_base}/v1/channels/{}",
                saved["channel_id"].as_str().unwrap()
            ))
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap();
        assert_eq!(echoed.status(), 200);
        let echoed: Value = echoed.json().await.unwrap();
        update["expected_version"] = echoed["version"].clone();
        update["durable_custom_execution"] = false.into();
        assert_eq!(
            Client::new()
                .put(format!(
                    "{old_client_base}/v1/channels/{}",
                    saved["channel_id"].as_str().unwrap()
                ))
                .bearer_auth("token-alice")
                .json(&update)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        stop_old.send(()).unwrap();
        task_old.await.unwrap();
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn durable_custom_creation_refuses_disabled_gate_missing_replay_or_noncustom_before_effects()
{
    for case in ["writer", "routing", "provider", "source"] {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut configured = state(&provider, &cma, &nyxid);
        let mut input = body(true);
        match case {
            "writer" => configured.custom_durable_execution_enabled = false,
            "routing" => configured.multi_consumer_writes_enabled = false,
            "provider" => {
                configured.providers =
                    management_state(&provider, &cma, &nyxid, key_ring(1)).providers
            }
            "source" => input["sources"] = create_body()["sources"].clone(),
            _ => unreachable!(),
        }
        let (base, stop, task) = serve_app(configured).await;
        assert_eq!(post(&base, &input).await.status(), 400, "{case}");
        assert!(provider.requests().is_empty(), "{case}");
        assert!(cma.records().is_empty(), "{case}");
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn unmarked_custom_creates_keep_legacy_wire_shape_and_false_replays_absence() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = serve_app(state(&provider, &cma, &nyxid)).await;
    // The older non-reserved creation API also stores routing and marker in one write.
    let response = post(&base, &body(true)).await;
    assert_eq!(response.status(), 201);
    let direct: Value = response.json().await.unwrap();
    assert_eq!(direct["definition"]["durable_custom_execution"], true);
    assert_eq!(
        direct["definition"]["routing"]["default_targets"],
        json!(["default"])
    );
    let mut input = body(false);
    input["record_id"] = uuid::Uuid::new_v4().to_string().into();
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let saved: Value = response.json().await.unwrap();
    assert!(saved["definition"]
        .get("durable_custom_execution")
        .is_none());
    assert!(saved["definition"].get("routing").is_none());
    input["durable_custom_execution"] = false.into();
    let replay: Value = post(&base, &input).await.json().await.unwrap();
    assert_eq!(replay["channel_id"], saved["channel_id"]);
    input["durable_custom_execution"] = true.into();
    assert_eq!(post(&base, &input).await.status(), 409);
    let permissions: Value = Client::new()
        .get(format!("{base}/v1/permissions"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        permissions["relay_authoring"]["custom_durable_execution_enabled"],
        true
    );
    assert_eq!(
        permissions["relay_authoring"]["response_recovery_providers"],
        json!(["cma_codex"])
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn new_existing_agent_media_contract_requires_ready_continuation_and_retains_when_writer_off()
{
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut configured = state(&provider, &cma, &nyxid);
    configured.media_enabled = true;
    let mut input = body(true);
    input["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_custom_media"});
    input["sources"][0]["attachments"] = json!({"pointer":"/attachments","origins":[]});
    let (base, stop, task) = serve_app(configured.clone()).await;
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 400);
    let refused: Value = response.json().await.unwrap();
    assert!(refused
        .to_string()
        .contains("custom_existing_agent_media_unavailable"));
    assert!(provider.requests().is_empty());
    assert!(cma.records().is_empty());
    stop.send(()).unwrap();
    task.await.unwrap();
    configured
        .custom_existing_agent_media_providers
        .insert("cma_codex".into());
    let (ready_base, stop, task) = serve_app(configured.clone()).await;
    let response = post(&ready_base, &input).await;
    let status = response.status();
    let created: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{created}");
    stop.send(()).unwrap();
    task.await.unwrap();
    configured.custom_existing_agent_media_providers.clear();
    configured.custom_durable_execution_enabled = false;
    let (off_base, stop, task) = serve_app(configured).await;
    let mut update = input;
    update["expected_version"] = created["version"].clone();
    update["name"] = "retained media configuration".into();
    update.as_object_mut().unwrap().remove("credentials");
    let response = Client::new()
        .put(format!(
            "{off_base}/v1/channels/{}",
            created["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let saved: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{saved}");
    assert_eq!(
        saved["definition"]["sources"],
        created["definition"]["sources"]
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
