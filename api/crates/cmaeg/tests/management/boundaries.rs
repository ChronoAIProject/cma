use super::*;

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
            "createChannel",
            "deleteChannel",
            "getChannel",
            "listChannels",
            "recoverChannel",
            "rekeyChannel",
            "testChannel",
            "updateChannel"
        ]
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
