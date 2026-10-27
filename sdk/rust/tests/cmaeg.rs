#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    cmaeg::{Cmaeg, ConnectionOptions, EventOptions, GatewayEvent, types::*},
};
use serde::Serialize;
use serde_json::{Value, json};
use wire::WireServer;

fn sample(name: &str) -> Value {
    let values: Value = serde_json::from_str(include_str!(
        "../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json"
    ))
    .unwrap();
    values[name][0].clone()
}
fn client(server: &WireServer) -> Cmaeg {
    let _ = (&server.client, &server.anonymous);
    Cmaeg::builder(&server.base_url)
        .unwrap()
        .bearer_token("sdk-fixture")
        .unwrap()
        .build()
        .unwrap()
}
fn same<T: Serialize>(value: T, expected: Value) {
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
}
fn event() -> CloudEvent {
    serde_json::from_value(json!({"specversion":"1.0","id":"event-1","source":"urn:fixture","type":"example.event","data":{"value":1},"traceid":"trace"})).unwrap()
}

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

#[test]
fn gateway_credential_debug_is_redacted() {
    assert_eq!(
        format!("{:?}", Secret("local-private-fixture".into())),
        "[redacted]"
    );
}

#[tokio::test]
async fn management_and_tools_emit_exact_methods_versions_credentials_and_opaque_segments() {
    let server = WireServer::new().await;
    let client = client(&server);
    let body: ChannelCreateRequest =
        serde_json::from_value(sample("ChannelCreateRequest")).unwrap();
    let mut request = wire::json(
        "POST",
        "/v1/channels",
        Some(serde_json::to_value(&body).unwrap()),
        201,
        sample("ChannelResponse"),
        true,
    );
    request
        .headers
        .insert("idempotency-key".into(), "keep-the-same-key".into());
    server.expect(request);
    same(
        client
            .create_channel(&body, Some("keep-the-same-key"))
            .await
            .unwrap(),
        sample("ChannelResponse"),
    );
    let id = "opaque/+?%value";
    let path = "/v1/channels/opaque%2F+%3F%25value";
    server.expect(wire::json(
        "GET",
        path,
        None,
        200,
        sample("ChannelResponse"),
        true,
    ));
    same(
        client.get_channel(id).await.unwrap(),
        sample("ChannelResponse"),
    );
    server.expect(wire::json(
        "GET",
        "/v1/channel-records/record",
        None,
        200,
        sample("ChannelResponse"),
        true,
    ));
    client.recover_channel("record").await.unwrap();
    let update: ChannelUpdateRequest =
        serde_json::from_value(sample("ChannelUpdateRequest")).unwrap();
    server.expect(wire::json(
        "PUT",
        path,
        Some(serde_json::to_value(&update).unwrap()),
        200,
        sample("ChannelResponse"),
        true,
    ));
    client.update_channel(id, &update).await.unwrap();
    for operation in ["test", "rekey"] {
        let schema = if operation == "test" {
            "TestChannelResponse"
        } else {
            "RekeyResponse"
        };
        server.expect(wire::json(
            "POST",
            &format!("{path}/{operation}"),
            None,
            200,
            sample(schema),
            true,
        ));
        if operation == "test" {
            same(client.test_channel(id).await.unwrap(), sample(schema));
        } else {
            same(client.rekey_channel(id).await.unwrap(), sample(schema));
        }
    }
    let message: OutboundMessage = serde_json::from_value(json!({"text":"hello"})).unwrap();
    server.expect(wire::json(
        "POST",
        "/v1/events/event%2Fref/replies",
        Some(json!({"text":"hello"})),
        200,
        sample("IntentReceipt"),
        true,
    ));
    same(
        client.reply_to_event("event/ref", &message).await.unwrap(),
        sample("IntentReceipt"),
    );
    server.expect(wire::json(
        "POST",
        "/v1/messages/message%2Fref/update",
        Some(json!({"text":"hello"})),
        200,
        sample("IntentReceipt"),
        true,
    ));
    client
        .update_message("message/ref", &message)
        .await
        .unwrap();
    let mut typing = wire::json(
        "POST",
        "/v1/events/event%2Fref/typing",
        None,
        200,
        sample("TypingReceipt"),
        true,
    );
    typing
        .headers
        .insert("content-type".into(), "application/json".into());
    server.expect(typing);
    client.send_typing("event/ref").await.unwrap();
    server.expect(wire::json(
        "GET",
        "/v1/events/event%2Fref/context",
        None,
        200,
        sample("ContextResponse"),
        true,
    ));
    same(
        client.read_event_context("event/ref").await.unwrap(),
        sample("ContextResponse"),
    );
    let mut request = wire::json(
        "DELETE",
        &format!("{path}?expected_version=42"),
        None,
        204,
        Value::Null,
        true,
    );
    request.response.clear();
    server.expect(request);
    client.delete_channel(id, 42).await.unwrap();
    server.assert_drained();
    assert_eq!(server.request_count(), 11);
}

#[tokio::test]
async fn custom_events_use_structured_media_type_and_return_the_receipt() {
    let server = WireServer::new().await;
    let mut request = wire::json(
        "POST",
        "/v1/channels/channel/events",
        Some(serde_json::to_value(event()).unwrap()),
        202,
        json!({"event_id":"event-1","conversation_id":"conversation","status":"accepted"}),
        true,
    );
    request
        .headers
        .insert("content-type".into(), "application/cloudevents+json".into());
    server.expect(request);
    let receipt = client(&server)
        .post_event("channel", &event(), &EventOptions::default())
        .await
        .unwrap();
    assert_eq!(receipt.event_id, "event-1");
    server.assert_drained();
}

fn frame(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}
fn completed() -> String {
    frame(
        "cmaeg.completed",
        json!({"event_id":"event-1","conversation_id":"conversation","text":"done","message":null,"error":null}),
    )
}
fn expect_stream(server: &WireServer, body: String) {
    let mut request = wire::json(
        "POST",
        "/v1/channels/channel/events?mode=stream",
        Some(serde_json::to_value(event()).unwrap()),
        200,
        Value::Null,
        true,
    );
    request
        .headers
        .insert("content-type".into(), "application/cloudevents+json".into());
    request
        .headers
        .insert("accept".into(), "text/event-stream".into());
    request
        .response_headers
        .insert("content-type".into(), "text/event-stream".into());
    request.response.clear();
    request.chunks = Some(body.as_bytes().chunks(7).map(<[u8]>::to_vec).collect());
    server.expect(request);
}

#[tokio::test]
async fn fragmented_streams_keep_utf8_and_end_without_reconnect() {
    let server = WireServer::new().await;
    expect_stream(
        &server,
        frame(
            "cmaeg.text.delta",
            json!({"event_id":"event-1","conversation_id":"conversation","text":"hello 🌍"}),
        ) + &completed(),
    );
    let mut stream = client(&server)
        .stream_event("channel", &event(), &EventOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(stream.next().await,Some(Ok(GatewayEvent::TextDelta(value))) if value.text == "hello 🌍")
    );
    assert!(matches!(
        stream.next().await,
        Some(Ok(GatewayEvent::Completed(_)))
    ));
    assert!(stream.next().await.is_none());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn streams_distinguish_terminal_failure_backpressure_wrong_identity_and_truncation() {
    for (body, expected) in [
        (
            frame(
                "cmaeg.failed",
                json!({"error":{"code":"internal","detail":"sync_backpressure"}}),
            ),
            "backpressure",
        ),
        (
            frame(
                "cmaeg.failed",
                json!({"event_id":"event-1","conversation_id":"conversation","text":null,"message":null,"error":{"title":"Unavailable","status":503,"code":"provider_unavailable"}}),
            ),
            "failed",
        ),
        (
            frame(
                "cmaeg.text.delta",
                json!({"event_id":"another","conversation_id":"conversation","text":"wrong"}),
            ),
            "protocol",
        ),
        (String::new(), "transport"),
        (format!("id: no-replay\n{}", completed()), "protocol"),
    ] {
        let server = WireServer::new().await;
        expect_stream(&server, body);
        let mut stream = client(&server)
            .stream_event("channel", &event(), &EventOptions::default())
            .await
            .unwrap();
        let result = stream.next().await;
        match expected {
            "backpressure" => assert!(matches!(result, Some(Ok(GatewayEvent::Backpressure)))),
            "failed" => assert!(matches!(result, Some(Ok(GatewayEvent::Failed(_))))),
            "protocol" => assert!(matches!(result, Some(Err(Error::Protocol { .. })))),
            "transport" => assert!(matches!(result, Some(Err(Error::Transport { .. })))),
            _ => unreachable!(),
        }
        assert!(stream.next().await.is_none());
        assert_eq!(server.request_count(), 1);
        server.assert_drained();
    }
}

#[tokio::test]
async fn stale_mutations_return_typed_problem_codes_without_retry() {
    let server = WireServer::new().await;
    server.expect(wire::json(
        "DELETE",
        "/v1/channels/channel?expected_version=1",
        None,
        409,
        json!({"status":409,"code":"conflict","detail":"stale version"}),
        true,
    ));
    assert!(
        matches!(client(&server).delete_channel("channel",1).await,Err(Error::Api { status:409,code:Some(code),.. }) if code == "conflict")
    );
    assert_eq!(server.request_count(), 1);
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

#[path = "cmaeg/outbound.rs"]
mod outbound;
#[tokio::test]
async fn native_effect_operations_preserve_reference_and_idempotency_without_retries() {
    let server = WireServer::new().await;
    let client = client(&server);
    server.expect(wire::json(
        "GET",
        "/v1/messages/opaque%2Fref/capabilities",
        None,
        200,
        sample("MessageEffectsCapabilities"),
        true,
    ));
    same(
        client
            .read_message_capabilities("opaque/ref")
            .await
            .unwrap(),
        sample("MessageEffectsCapabilities"),
    );
    let mut expected = wire::json(
        "POST",
        "/v1/messages/opaque%2Fref/delete",
        Some(json!({})),
        200,
        sample("MessageEffectReceipt"),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "effect-delete".into());
    server.expect(expected);
    same(
        client
            .delete_message("opaque/ref", "effect-delete")
            .await
            .unwrap(),
        sample("MessageEffectReceipt"),
    );
    let body: MessageReactionRequest =
        serde_json::from_value(sample("MessageReactionRequest")).unwrap();
    let mut expected = wire::json(
        "POST",
        "/v1/messages/opaque%2Fref/reaction",
        Some(serde_json::to_value(&body).unwrap()),
        200,
        sample("MessageEffectReceipt"),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "effect-reaction".into());
    server.expect(expected);
    same(
        client
            .set_message_reaction("opaque/ref", "effect-reaction", &body)
            .await
            .unwrap(),
        sample("MessageEffectReceipt"),
    );
    server.assert_drained();
}

#[tokio::test]
async fn explicit_thread_history_keeps_the_original_event_destination() {
    let server = WireServer::new().await;
    let client = client(&server);
    server.expect(wire::json(
        "GET",
        "/v1/events/original/context?include_thread_history=true",
        None,
        200,
        sample("ContextResponse"),
        true,
    ));
    same(
        client
            .read_event_context_with_history("original")
            .await
            .unwrap(),
        sample("ContextResponse"),
    );
}
