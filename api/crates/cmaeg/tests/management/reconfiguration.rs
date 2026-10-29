//! Exercise the real Gateway HTTP boundary around CMA's binding refusal.
//! Core binding continuity tests separately prove the Manager's immutable guard.
use super::*;
use serde_json::Value;

#[tokio::test]
async fn refused_profile_scope_expansion_preserves_the_channel_and_allows_unchanged_retry() {
    let refusal = json!({
        "code":"binding_reconfiguration_unsupported",
        "detail":"private upstream credential and connection endpoint must not leak"
    });
    let provider = FakeProvider::start(FakeProviderConfig {
        // Create and the first unchanged PUT succeed, then six expansions fail.
        // The final unchanged PUT must still reach the provider and succeed.
        binding_failures: (2..8)
            .map(|attempt| (attempt, (409, refusal.clone())))
            .collect(),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let mut input = create_body();
    input["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"h1"}}]);
    input["partition"] = json!({"version":1,"key":"per_event"});
    input["reply"]["sinks"] = json!([{"type":"sync"}]);
    input["profile"]["permission"]["service_scope"] = json!(["llm-public", "ornn-api"]);
    input["profile"]["environment"] = json!({
        "size":{"cpu_cores":2,"memory_gib":8},
        "workspace":{"type":"directory","name":"work"},
        "setup_commands":[],"environment_variables":[]
    });
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
    let mut unchanged = input;
    unchanged.as_object_mut().unwrap().remove("credentials");
    unchanged["expected_version"] = initial["version"].clone();
    let response = client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&unchanged)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let saved: Value = response.json().await.unwrap();
    assert_eq!(saved["version"], 2);
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
            let response = client
                .put(&url)
                .bearer_auth("token-alice")
                .json(&update)
                .send()
                .await
                .unwrap();
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
            assert!(detail.contains("have not been changed"));
            assert!(!problem.to_string().contains("private upstream"));
            assert!(!problem.to_string().contains("nyx_agent_secret"));

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

    let response = client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&unchanged)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let retried: Value = response.json().await.unwrap();
    assert_eq!(retried["version"], 3);
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
    assert_eq!(requests.len(), 9);
    assert!(requests
        .iter()
        .all(|request| request.method == "PUT" && request.path == requests[0].path));
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(requests[0].body, requests[8].body);
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
