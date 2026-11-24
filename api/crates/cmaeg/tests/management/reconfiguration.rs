//! Exercise the real Gateway HTTP boundary for pinned CMA binding scope.
//! Core binding continuity tests separately prove the Manager's immutable guard.
use super::*;
use serde_json::Value;

#[tokio::test]
async fn refused_profile_scope_expansion_preserves_the_channel_and_allows_unchanged_retry() {
    scope_roundtrip(None, false).await;
}

#[tokio::test]
async fn retained_private_connection_scope_succeeds_before_and_after_refused_expansion() {
    for service in [
        "retained-private-service",
        "aa480b39-3ba0-42af-9c21-10d02885337c",
    ] {
        scope_roundtrip(Some(service), true).await;
    }
}

#[tokio::test]
async fn scope_expansion_respects_an_uncertain_binding_mutation() {
    let provider = FakeProvider::start(FakeProviderConfig {
        binding_failures: [(1, (502, json!({})))].into(),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
    enable_custody(&mut state, &cma);
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let (url, mut update) = create_scope_channel(&client, &base, create_body()).await;

    let unknown = put_channel(&client, &url, &update).await;
    assert_eq!(unknown.status(), 503);
    let held = cma.records();
    assert_eq!(held.len(), 1);
    let initial_version = update["expected_version"].as_u64().unwrap();
    assert!(held.values().all(|value| value.version > initial_version));

    update["profile"]["permission"]["service_scope"] = json!(["private-expansion"]);
    let response = put_channel(&client, &url, &update).await;
    assert_eq!(response.status(), 409);
    assert_eq!(response.json::<Value>().await.unwrap()["code"], "conflict");
    assert_eq!(provider.requests().len(), 2);
    for (id, previous) in held {
        let current = &cma.records()[&id];
        assert_eq!(current.version, previous.version);
        assert_eq!(current.blob, previous.blob);
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn third_party_provider_and_fixed_agent_keep_their_scope_update_contracts() {
    for fixed in [false, true] {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        let mut input = create_body();
        if fixed {
            input["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_fixture"});
        } else {
            state.providers = Arc::new(
                ProviderClient::new(
                    registry_for(provider.base_url(), "external"),
                    std::time::Duration::from_secs(1),
                    std::time::Duration::from_secs(2),
                    8,
                )
                .unwrap(),
            );
            input["provider"]["slug"] = "external".into();
            input["profile"]["metadata"]["provider"] = "external".into();
        }
        let (base, stop, task) = serve_app(state).await;
        let client = Client::new();
        let (url, mut update) = create_scope_channel(&client, &base, input).await;
        update["profile"]["permission"]["service_scope"] = json!(["private-expansion"]);
        let response = put_channel(&client, &url, &update).await;
        assert_eq!(response.status(), 200, "fixed target: {fixed}");
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        let bind: Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(
            bind["profile"]["permission"]["service_scope"],
            json!(["private-expansion"])
        );

        stop.send(()).unwrap();
        task.await.unwrap();
        provider.close().await;
        cma.close().await;
        nyxid.close().await;
    }
}

fn enable_custody(state: &mut ManagementState, cma: &FakeCma) {
    state.records = Arc::new(
        RecordClient::new(&cma.base_url, "cma-service")
            .unwrap()
            .with_channel_mutation_custody(true),
    );
}

async fn create_scope_channel(client: &Client, base: &str, mut input: Value) -> (String, Value) {
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let initial: Value = response.json().await.unwrap();
    let url = format!(
        "{base}/v1/channels/{}",
        initial["channel_id"].as_str().unwrap()
    );
    input.as_object_mut().unwrap().remove("record_id");
    input.as_object_mut().unwrap().remove("credentials");
    input["expected_version"] = initial["version"].clone();
    (url, input)
}

async fn put_channel(client: &Client, url: &str, body: &Value) -> reqwest::Response {
    client
        .put(url)
        .bearer_auth("token-alice")
        .json(body)
        .send()
        .await
        .unwrap()
}

async fn scope_roundtrip(retained_private_service: Option<&str>, custody: bool) {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
    if custody {
        enable_custody(&mut state, &cma);
    }
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let mut input = create_body();
    input["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"h1"}}]);
    input["partition"] = json!({"version":1,"key":"per_event"});
    input["reply"]["sinks"] = json!([{"type":"sync"}]);
    input["profile"]["permission"]["service_scope"] = json!(["llm-public", "ornn-api"]);
    if let Some(service) = retained_private_service {
        input["profile"]["permission"]["service_scope"]
            .as_array_mut()
            .unwrap()
            .push(service.into());
    }
    input["profile"]["environment"] = json!({
        "size":{"cpu_cores":2,"memory_gib":8},
        "workspace":{"type":"directory","name":"work"},
        "setup_commands":[],"environment_variables":[]
    });
    let (url, mut unchanged) = create_scope_channel(&client, &base, input).await;
    let response = put_channel(&client, &url, &unchanged).await;
    assert_eq!(response.status(), 200);
    let saved: Value = response.json().await.unwrap();
    assert!(saved["version"].as_u64().unwrap() > unchanged["expected_version"].as_u64().unwrap());
    unchanged["expected_version"] = saved["version"].clone();
    let retained = cma.records();

    for service in [
        "personal-custom-service",
        "106f2888-6bb9-4ff1-a8d8-249af96c06ac",
        "github",
    ] {
        for explicit_key in [false, true] {
            let mut update = unchanged.clone();
            update["profile"]["permission"]["service_scope"]
                .as_array_mut()
                .unwrap()
                .push(service.into());
            if explicit_key {
                update["credentials"] = json!({"agent_key":"nyx_agent_secret"});
            }
            let response = put_channel(&client, &url, &update).await;
            assert_eq!(
                response.status(),
                409,
                "{service}, explicit_key={explicit_key}"
            );
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
            let problem: Value = response.json().await.unwrap();
            assert_eq!(problem["code"], "provider_rejected");
            assert_eq!(problem["status"], 409);
            let detail = problem["detail"].as_str().unwrap();
            assert!(detail.contains("service scope"));
            assert!(detail.contains("create a new binding with the required scope"));
            assert!(detail.contains("Keep the current profile"));
            assert!(detail.contains("have not been changed"));
            assert!(!problem.to_string().contains("private upstream"));
            assert!(!problem.to_string().contains("nyx_agent_secret"));
            assert_eq!(provider.requests().len(), 2);

            let current = cma.records();
            assert_eq!(current.len(), retained.len());
            for (id, previous) in &retained {
                let current = current.get(id).unwrap();
                assert_eq!(current.version, previous.version);
                assert_eq!(current.owner, previous.owner);
                // Exact encrypted bytes include the retained scope and credentials.
                assert_eq!(current.blob, previous.blob);
            }
            let response = client
                .get(&url)
                .bearer_auth("token-alice")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let loaded: Value = response.json().await.unwrap();
            assert_eq!(loaded["version"], saved["version"]);
            assert_eq!(loaded["definition"], saved["definition"]);
        }
    }

    let response = put_channel(&client, &url, &unchanged).await;
    assert_eq!(response.status(), 200);
    let retried: Value = response.json().await.unwrap();
    assert!(retried["version"].as_u64().unwrap() > saved["version"].as_u64().unwrap());
    assert_eq!(retried["channel_id"], saved["channel_id"]);
    assert_eq!(
        retried["definition"]["profile"],
        saved["definition"]["profile"]
    );
    assert_eq!(
        retried["definition"]["provider"],
        saved["definition"]["provider"]
    );
    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests
        .iter()
        .all(|request| request.method == "PUT" && request.path == requests[0].path));
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(requests[0].body, requests[2].body);
    for request in requests {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert!(
            body.get("target").is_none(),
            "profile-managed binding required"
        );
        assert_eq!(body["agent_key"], "nyx_agent_secret");
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
