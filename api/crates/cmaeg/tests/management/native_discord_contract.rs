//! NyxID #2183 / CMA #1340: routing belongs on the policy endpoint.
use super::*;

fn routing() -> Value {
    json!({
        "version":1,"default_consumer":{"name":"default","mentionable":false},
        "default_targets":["default"],"max_targets":1,"reply_label":"never",
        "addressing":{"mentions":false,"command":false,"event_field":false,
            "unknown_target_notice":false}
    })
}

fn native_body() -> Value {
    let mut input = body();
    input["provider"]["slug"] = json!("nyxbot");
    input["profile"]["metadata"]["provider"] = json!("nyxbot");
    input["sources"][0]["admission"] = json!({
        "type":"scoped","senders":{"type":"open"},
        "chats":{"type":"open"},"groups":"all"
    });
    input["partition"] = json!({"version":1,"key":"conversation_and_sender"});
    input["reply"] = json!({"auto_final":true,"ack_after_ms":0,"sinks":[{"type":"source"}]});
    input
}

async fn put(base: &str, path: &str, input: &Value) -> reqwest::Response {
    Client::new()
        .put(format!("{base}/v1/channels/{path}"))
        .bearer_auth("token-alice")
        .json(input)
        .send()
        .await
        .unwrap()
}

async fn assert_routing_refused(response: reqwest::Response) {
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(problem["detail"], "/routing: not accepted in channel create or full update; use PUT /v1/channels/{channel_id}/routing with {expected_version,routing} after creation");
}

#[tokio::test]
async fn native_create_rejects_misplaced_routing_before_provider_or_record_effects() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) =
        serve_app(native_state_with_slug(&provider, &cma, &nyxid, "nyxbot")).await;
    for policy in [routing(), Value::Null, json!("private-submitted-value")] {
        let mut input = native_body();
        input["routing"] = policy;
        assert_routing_refused(post(&base, &input).await).await;
        assert!(cma.records().is_empty());
        assert!(provider.requests().is_empty());
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn native_create_then_policy_update_accepts_nyxid_shape_and_full_update_preserves_it() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) =
        serve_app(native_state_with_slug(&provider, &cma, &nyxid, "nyxbot")).await;
    let mut input = native_body();
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(
        created["native_discord_activation"]["state"],
        "pending_discovery"
    );
    let id = created["channel_id"].as_str().unwrap();
    let binds = provider.requests().len();
    // NyxID retains record_id when a create response is lost. Retrying the
    // activation must recover the same channel without another provider bind.
    let retried_create = post(&base, &input).await;
    assert_eq!(retried_create.status(), 201);
    let retried_create: Value = retried_create.json().await.unwrap();
    assert_eq!(retried_create["channel_id"], created["channel_id"]);
    assert_eq!(retried_create["record_id"], created["record_id"]);
    assert_eq!(retried_create["version"], created["version"]);
    for field in ["slug", "binding_id", "level"] {
        assert_eq!(
            retried_create["provider"][field],
            created["provider"][field]
        );
    }
    assert_eq!(cma.records().len(), 1);
    assert_eq!(provider.requests().len(), binds);
    let policy = json!({"expected_version":created["version"],"routing":routing()});
    let response = put(&base, &format!("{id}/routing"), &policy).await;
    assert_eq!(response.status(), 200);
    let routed: Value = response.json().await.unwrap();
    assert_eq!(routed["definition"]["routing"], routing());
    assert_eq!(provider.requests().len(), binds);
    assert_eq!(
        routed["provider"]["binding_id"],
        created["provider"]["binding_id"]
    );

    // An exact retry after a lost policy response confirms the saved version.
    let retried = put(&base, &format!("{id}/routing"), &policy).await;
    assert_eq!(retried.status(), 200);
    assert_eq!(
        retried.json::<Value>().await.unwrap()["version"],
        routed["version"]
    );
    let mut conflicting = policy.clone();
    conflicting["routing"]["max_targets"] = json!(2);
    assert_eq!(
        put(&base, &format!("{id}/routing"), &conflicting)
            .await
            .status(),
        409
    );

    input.as_object_mut().unwrap().remove("record_id");
    input.as_object_mut().unwrap().remove("credentials");
    input["expected_version"] = routed["version"].clone();
    input["routing"] = routing();
    assert_routing_refused(put(&base, id, &input).await).await;
    assert_eq!(provider.requests().len(), binds);
    assert_eq!(
        cma.records().values().next().unwrap().version,
        routed["version"].as_u64().unwrap()
    );

    input.as_object_mut().unwrap().remove("routing");
    let updated = put(&base, id, &input).await;
    assert_eq!(updated.status(), 200);
    let updated: Value = updated.json().await.unwrap();
    assert_eq!(updated["definition"]["routing"], routing());
    assert_eq!(
        updated["definition"]["sources"],
        created["definition"]["sources"]
    );
    assert!(!updated.to_string().contains("native-owner-private-token"));
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn unrelated_schema_errors_remain_generic_and_never_echo_submitted_data() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) =
        serve_app(native_state_with_slug(&provider, &cma, &nyxid, "nyxbot")).await;
    let mut cases = Vec::new();
    for (pointer, value) in [
        ("/profile/routing", json!("private-value")),
        ("/credentials/routing", json!("private-value")),
        ("/private-unknown-field", json!("private-value")),
        ("/sources", json!([{"type":"private-unknown-variant"}])),
    ] {
        let mut input = native_body();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        input.pointer_mut(parent).unwrap()[field] = value;
        cases.push(input.to_string());
    }
    cases.push("{\"name\":\"private-value\"".into());
    for input in cases {
        let response = Client::new()
            .post(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .header("content-type", "application/json")
            .body(input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        let problem: Value = response.json().await.unwrap();
        assert_eq!(problem["code"], "invalid_request");
        assert_eq!(
            problem["detail"],
            "request does not match the channel schema"
        );
        assert!(!problem.to_string().contains("private-"));
    }
    assert!(cma.records().is_empty());
    assert!(provider.requests().is_empty());
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn scoped_discord_admission_update_is_versioned_and_readable() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) =
        serve_app(native_state_with_slug(&provider, &cma, &nyxid, "nyxbot")).await;
    let mut input = native_body();
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    let id = created["channel_id"].as_str().unwrap();
    let original_version = created["version"].as_u64().unwrap();
    input.as_object_mut().unwrap().remove("record_id");
    input.as_object_mut().unwrap().remove("credentials");
    input["expected_version"] = original_version.into();
    let scoped = json!({
        "type":"scoped","senders":{"type":"allowlist","ids":["42"]},
        "chats":{"type":"allowlist","ids":["100"]},"groups":"all"
    });
    input["sources"][0]["admission"] = scoped.clone();
    let response = put(&base, id, &input).await;
    assert_eq!(response.status(), 200);
    let updated: Value = response.json().await.unwrap();
    let version = updated["version"].as_u64().unwrap();
    assert!(version > original_version);
    assert_eq!(updated["definition"]["sources"][0]["admission"], scoped);
    assert_eq!(
        updated["provider"]["binding_id"],
        created["provider"]["binding_id"]
    );
    assert_eq!(
        updated["definition"]["routing"],
        created["definition"]["routing"]
    );
    assert_eq!(updated["channel_id"], created["channel_id"]);

    let mut denied = scoped.clone();
    denied["chats"] = json!({"type":"deny"});
    input["sources"][0]["admission"] = denied.clone();
    assert_eq!(put(&base, id, &input).await.status(), 409);

    let read = Client::new()
        .get(format!("{base}/v1/channels/{id}"))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    let read: Value = read.json().await.unwrap();
    assert_eq!(read["version"], version);
    assert_eq!(read["definition"]["sources"][0]["admission"], scoped);

    input["expected_version"] = version.into();
    for dimension in ["senders", "chats"] {
        input["sources"][0]["admission"] = scoped.clone();
        input["sources"][0]["admission"][dimension] = json!({"type":"allowlist","ids":[]});
        let invalid = put(&base, id, &input).await;
        assert_eq!(invalid.status(), 400);
        assert_eq!(cma.records().values().next().unwrap().version, version);
    }
    input["sources"][0]["admission"] = denied.clone();
    let response = put(&base, id, &input).await;
    assert_eq!(response.status(), 200);
    let saved: Value = response.json().await.unwrap();
    assert!(saved["version"].as_u64().unwrap() > version);
    assert_eq!(saved["definition"]["sources"][0]["admission"], denied);
    stop.send(()).unwrap();
    task.await.unwrap();
}
