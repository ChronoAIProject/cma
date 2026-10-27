use super::*;

#[tokio::test]
async fn current_subject_resource_discovery_has_no_selected_owner_or_originating_agent_credential()
{
    let server = WireServer::new().await;
    let client = client(&server);
    for (platform, name) in [
        (ResourcePlatform::Lark, "lark"),
        (ResourcePlatform::Feishu, "feishu"),
    ] {
        let expected = json!({"items":[{"user_service_id":"connection","catalog_service_id":"catalog","name":"Research bot","platform":name,"source":"organization"}]});
        server.expect(wire::json(
            "GET",
            &format!("/v1/relay-resource-connections?platform={name}"),
            None,
            200,
            expected.clone(),
            true,
        ));
        same(
            client
                .list_relay_resource_connections(platform)
                .await
                .unwrap(),
            expected,
        );
    }
    server.assert_drained();
}

#[tokio::test]
async fn connection_operations_keep_owner_and_origin_authority_local_to_the_request() {
    let server = WireServer::new().await;
    let client = client(&server);
    let org = ConnectionOptions {
        owner: Some("org-id"),
    };
    server.expect(wire::json(
        "GET",
        "/v1/channel-capabilities",
        None,
        200,
        sample("CapabilityView"),
        true,
    ));
    same(
        client.get_channel_capabilities().await.unwrap(),
        sample("CapabilityView"),
    );
    let body: CreateConnection = serde_json::from_value(sample("CreateConnection")).unwrap();
    let mut expected = wire::json(
        "POST",
        "/v1/channel-connections",
        Some(sample("CreateConnection")),
        201,
        sample("ConnectionView"),
        true,
    );
    expected
        .headers
        .insert("x-cmaeg-owner".into(), "org-id".into());
    expected.headers.insert(
        "x-cmaeg-agent-authorization".into(),
        "Bearer origin-agent".into(),
    );
    server.expect(expected);
    same(
        client
            .create_channel_connection(&body, "origin-agent", &org)
            .await
            .unwrap(),
        sample("ConnectionView"),
    );
    server.expect(wire::json(
        "GET",
        "/v1/channel-connections?after=cursor%2F%2B%3F",
        None,
        200,
        json!({"items":[],"next_after":"next"}),
        true,
    ));
    let page = client
        .list_channel_connections(Some("cursor/+?"), &ConnectionOptions::default())
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.next_after.as_deref(), Some("next"));
    let path = "/v1/channel-connections/opaque%2F+%3F";
    server.expect(wire::json(
        "GET",
        path,
        None,
        200,
        sample("ConnectionView"),
        true,
    ));
    same(
        client
            .get_channel_connection("opaque/+?", &ConnectionOptions::default())
            .await
            .unwrap(),
        sample("ConnectionView"),
    );
    let mut expected = wire::json("DELETE", path, None, 204, Value::Null, true);
    expected.response.clear();
    expected
        .headers
        .insert("x-cmaeg-owner".into(), "org-id".into());
    server.expect(expected);
    client
        .cancel_channel_connection("opaque/+?", &org)
        .await
        .unwrap();
    assert!(matches!(
        client
            .create_channel_connection(&body, "bad\nkey", &org)
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 5);
    server.assert_drained();
}
