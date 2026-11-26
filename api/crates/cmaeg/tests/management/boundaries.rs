use super::*;

#[tokio::test]
async fn worker_ownership_is_bound_to_the_authenticated_creator_before_side_effects() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let url = format!("{base}/v1/channels");
    for reserved in [false, true] {
        for (token, kind, subject) in [
            ("token-alice", "human", "bob"),
            ("token-alice", "organization", "alice"),
            ("token-org-a", "organization", "org-b"),
            ("token-org-a", "human", "org-a"),
        ] {
            let mut body = create_body();
            body["profile"]["metadata"]["owner"] = json!({"kind": kind, "subject": subject});
            if reserved {
                body["record_id"] = uuid::Uuid::new_v4().to_string().into();
            }
            expect_problem(
                client
                    .post(&url)
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
                    .unwrap(),
                400,
            )
            .await;
            assert!(provider.requests().is_empty());
            assert!(cma.records().is_empty());
        }
    }
    for (token, kind, subject) in [
        ("token-alice", "human", "alice"),
        ("token-org-a", "organization", "org-a"),
    ] {
        let mut body = create_body();
        body["profile"]["metadata"]["owner"] = json!({"kind": kind, "subject": subject});
        let response = client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let created: serde_json::Value = response.json().await.unwrap();
        let id = created["channel_id"].as_str().unwrap();
        let record_id = created["record_id"].as_str().unwrap();
        let retained = cma.records().remove(record_id).unwrap();
        let calls = provider.requests().len();
        body.as_object_mut().unwrap().remove("credentials");
        body["expected_version"] = created["version"].clone();
        for replacement in [
            json!({"kind":kind,"subject":"another-owner"}),
            json!({"kind": if kind == "human" { "organization" } else { "human" }, "subject": subject}),
        ] {
            body["profile"]["metadata"]["owner"] = replacement;
            expect_problem(
                client
                    .put(format!("{url}/{id}"))
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
                    .unwrap(),
                400,
            )
            .await;
            assert_eq!(provider.requests().len(), calls);
            let after = cma.records().remove(record_id).unwrap();
            assert_eq!(after.version, retained.version);
            assert_eq!(after.blob, retained.blob);
        }
        if kind == "human" {
            for personal_kind in ["service_account", "agent_key", "human"] {
                body["profile"]["metadata"]["owner"] =
                    json!({"kind": personal_kind, "subject": subject});
                let updated = client
                    .put(format!("{url}/{id}"))
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(updated.status(), 200);
                let updated: serde_json::Value = updated.json().await.unwrap();
                body["expected_version"] = updated["version"].clone();
            }
        }
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

async fn expect_problem(response: reqwest::Response, status: u16) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], status);
    assert!(body["code"].is_string());
    assert!(!body.to_string().contains("nyx_agent_secret"));
}

#[tokio::test]
async fn http_errors_and_openapi_share_the_management_contract() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let url = format!("{base}/v1/channels");
    expect_problem(
        client.post(&url).json(&create_body()).send().await.unwrap(),
        401,
    )
    .await;
    expect_problem(client.get(&url).send().await.unwrap(), 401).await;
    expect_problem(client.patch(&url).send().await.unwrap(), 405).await;
    expect_problem(
        client.get(format!("{base}/absent")).send().await.unwrap(),
        404,
    )
    .await;
    expect_problem(
        client
            .post(&url)
            .bearer_auth("token-alice")
            .header("content-type", "application/json")
            .body("{nyx_agent_secret}")
            .send()
            .await
            .unwrap(),
        400,
    )
    .await;
    expect_problem(
        client
            .post(&url)
            .bearer_auth("token-alice")
            .header("content-type", "application/json")
            .body("x".repeat(1_048_577))
            .send()
            .await
            .unwrap(),
        413,
    )
    .await;
    let mut invalid = create_body();
    invalid["provider"]["slug"] = "unknown".into();
    invalid["profile"]["metadata"]["provider"] = "unknown".into();
    expect_problem(
        client
            .post(&url)
            .bearer_auth("token-alice")
            .json(&invalid)
            .send()
            .await
            .unwrap(),
        400,
    )
    .await;
    assert!(provider.requests().is_empty());
    let initial = created(&client, &base).await;
    let channel_id = initial["channel_id"].as_str().unwrap();
    let mut update = create_body();
    update["expected_version"] = 1.into();
    update["partition"]["version"] = 99.into();
    expect_problem(
        client
            .put(format!("{url}/{channel_id}"))
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap(),
        400,
    )
    .await;
    assert_eq!(provider.requests().len(), 1);
    let mut parts: Vec<_> = channel_id.split('.').map(str::to_owned).collect();
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut bytes = URL_SAFE_NO_PAD.decode(&parts[3]).unwrap();
    bytes[12] ^= 1;
    parts[3] = URL_SAFE_NO_PAD.encode(bytes);
    for (id, status) in [(parts.join("."), 404), ("cmaeg1.ch.invalid".into(), 400)] {
        expect_problem(
            client
                .get(format!("{url}/{id}"))
                .bearer_auth("token-alice")
                .send()
                .await
                .unwrap(),
            status,
        )
        .await;
    }
    let mut pointer = cmaeg_core::ids::ChannelId::new(channel_id)
        .unwrap()
        .open(&key_ring(1))
        .unwrap()
        .payload;
    pointer.record_url = format!(
        "https://another-origin.example/api/v1/gateway/channel-records/{}",
        pointer.record_id
    );
    let other_origin = pointer.seal_id(&key_ring(1)).unwrap();
    expect_problem(
        client
            .get(format!("{url}/{other_origin}"))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap(),
        404,
    )
    .await;
    let document: serde_json::Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reset = &document["paths"]["/channels/{channel_id}/partition/resets"]["post"];
    assert_eq!(reset["security"], json!([{"creatorBearer":[]}]));
    assert_eq!(reset["requestBody"]["required"], true);
    let key = reset["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Idempotency-Key")
        .unwrap();
    assert_eq!(key["required"], true);
    assert_eq!(key["schema"]["maxLength"], 128);
    assert!(document["paths"]
        .get("/v1/channels/{channel_id}/partition/resets")
        .is_none());
    let mut operations: Vec<_> = document["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|path| path.as_object().unwrap().values())
        .filter(|operation| {
            operation["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|tag| tag == "channels"))
        })
        .map(|operation| {
            assert!(
                operation["responses"]["default"]["content"]["application/problem+json"]
                    .is_object()
            );
            assert!(operation["security"].is_array());
            operation["operationId"].as_str().unwrap()
        })
        .collect();
    operations.sort();
    assert_eq!(
        operations,
        [
            "baselineChannelProfileRevisions",
            "cancelChannelCreate",
            "cancelCreateCapability",
            "createChannel",
            "deleteChannel",
            "getChannel",
            "getChannelActivity",
            "getChannelActivityHistory",
            "getChannelActivityHistoryDetails",
            "getChannelRecordManagement",
            "getEventMediaResponses",
            "inspectChannelPartition",
            "listChannels",
            "publishChannelProfileRevision",
            "recoverChannel",
            "rekeyChannel",
            "resetChannelPartition",
            "retireLegacyChannelRecord",
            "streamChannelActivity",
            "testChannel",
            "testChannelConsumer",
            "testChannelSource",
            "updateChannel",
            "updateChannelRouting"
        ]
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn retained_media_response_metadata_requires_the_channel_owner() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let initial = created(&client, &base).await;
    let channel_id = initial["channel_id"].as_str().unwrap();
    let url = format!("{base}/v1/channels/{channel_id}/events/event_private/media-responses");
    let calls = provider.requests().len();
    expect_problem(client.get(&url).send().await.unwrap(), 401).await;
    expect_problem(
        client
            .get(&url)
            .bearer_auth("token-bob")
            .send()
            .await
            .unwrap(),
        404,
    )
    .await;
    expect_problem(
        client
            .get(&url)
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap(),
        404,
    )
    .await;
    assert_eq!(
        provider.requests().len(),
        calls,
        "metadata lookup must never execute provider work"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn synthetic_turns_refuse_truncation_and_malformed_events() {
    for config in [
        FakeProviderConfig {
            disconnect_after: Some(2),
            ..Default::default()
        },
        FakeProviderConfig {
            malformed_frame: true,
            ..Default::default()
        },
    ] {
        let provider = FakeProvider::start(config).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (base, stop, task) = app(&provider, &cma, &nyxid).await;
        let client = Client::new();
        let initial = created(&client, &base).await;
        let url = format!(
            "{base}/v1/channels/{}/test",
            initial["channel_id"].as_str().unwrap()
        );
        expect_problem(
            client
                .post(url)
                .bearer_auth("token-alice")
                .send()
                .await
                .unwrap(),
            503,
        )
        .await;
        stop.send(()).unwrap();
        task.await.unwrap();
        provider.close().await;
        cma.close().await;
        nyxid.close().await;
    }
}

#[tokio::test]
async fn reset_validation_and_owner_failures_never_reach_provider() {
    use super::partition_resets::{body, invalid_response, request, response};
    let provider = FakeProvider::start(Default::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let channel = created(&client, &base).await;
    let id = channel["channel_id"].as_str().unwrap();
    let before = provider.log();
    invalid_response(request(&client, &base, id).json(&body())).await;
    for key in [
        "".into(),
        "a b".into(),
        "a\tb".into(),
        "a".repeat(129),
        "é".into(),
    ] {
        invalid_response(
            request(&client, &base, id)
                .header("Idempotency-Key", key)
                .json(&body()),
        )
        .await;
    }
    invalid_response(
        request(&client, &base, id)
            .header("Idempotency-Key", "one")
            .header("Idempotency-Key", "two")
            .json(&body()),
    )
    .await;
    for (field, value) in [
        ("partition_key", json!("")),
        ("partition_key", json!("é".repeat(4097))),
        ("expected_agent_id", json!("")),
        ("expected_agent_id", json!("é".repeat(65))),
        ("expected_version", json!(0)),
        ("expected_version", json!(-1)),
        ("consumer_id", json!("bad")),
        ("consumer_id", json!("csm_00000000000f")),
        ("profile", json!({})),
        ("service_scope", json!([])),
        ("owner", json!("bob")),
    ] {
        let mut invalid = body();
        invalid[field] = value;
        invalid_response(
            request(&client, &base, id)
                .header("Idempotency-Key", "validation")
                .json(&invalid),
        )
        .await;
    }
    for field in ["partition_key", "expected_version", "expected_agent_id"] {
        let mut invalid = body();
        invalid.as_object_mut().unwrap().remove(field);
        invalid_response(
            request(&client, &base, id)
                .header("Idempotency-Key", "validation")
                .json(&invalid),
        )
        .await;
    }
    let mut stale = body();
    stale["expected_version"] = 2.into();
    response(
        request(&client, &base, id)
            .header("Idempotency-Key", "stale")
            .json(&stale)
            .send()
            .await
            .unwrap(),
        409,
    )
    .await;
    for (token, status) in [("token-bob", 404), ("token-org-a", 404), ("invalid", 401)] {
        response(
            client
                .post(format!("{base}/v1/channels/{id}/partition/resets"))
                .bearer_auth(token)
                .header("Idempotency-Key", "owner")
                .json(&body())
                .send()
                .await
                .unwrap(),
            status,
        )
        .await;
    }
    assert_eq!(provider.log(), before);
    stop.send(()).unwrap();
    task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
