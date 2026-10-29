#[path = "cmaeg/connections.rs"]
mod connections;

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

#[tokio::test]
async fn thread_lifecycle_uses_exact_scoped_routes_and_never_retries_unknown_writes() {
    let server = WireServer::new().await;
    let client = client(&server);
    let listing =
        json!({"threads":[],"truncated":false,"scope":"public_children_of_original_parent"});
    server.expect(wire::json(
        "GET",
        "/v1/events/opaque%2Fevent/threads?view=archived",
        None,
        200,
        listing.clone(),
        true,
    ));
    same(
        client
            .list_event_threads("opaque/event", ThreadListView::Archived)
            .await
            .unwrap(),
        listing,
    );
    let create = CreateThreadRequest {
        name: "Review".into(),
        auto_archive_duration: Some(1440),
    };
    let unknown = json!({"outcome":{"status":"unknown","reason":"thread_write_unconfirmed"}});
    let mut request = wire::json(
        "POST",
        "/v1/events/opaque%2Fevent/threads",
        Some(serde_json::to_value(&create).unwrap()),
        200,
        unknown.clone(),
        true,
    );
    request
        .headers
        .insert("idempotency-key".into(), "create-stable".into());
    server.expect(request);
    same(
        client
            .create_event_thread("opaque/event", "create-stable", &create)
            .await
            .unwrap(),
        unknown,
    );
    let manage = ManageThreadRequest {
        target: ThreadTarget::Thread {
            thread_ref: "cmaeg1.tr.original".into(),
        },
        action: ThreadAction::Leave,
        name: None,
    };
    let confirmed = json!({"outcome":{"status":"confirmed"}});
    let mut request = wire::json(
        "POST",
        "/v1/thread-operations",
        Some(serde_json::to_value(&manage).unwrap()),
        200,
        confirmed.clone(),
        true,
    );
    request
        .headers
        .insert("idempotency-key".into(), "leave-stable".into());
    server.expect(request);
    same(
        client.manage_thread("leave-stable", &manage).await.unwrap(),
        confirmed,
    );
    let rename = ManageThreadRequest {
        target: manage.target,
        action: ThreadAction::Rename,
        name: Some("Incident triage".into()),
    };
    let unknown = json!({"outcome":{"status":"unknown","reason":"thread_write_unconfirmed"}});
    let mut request = wire::json(
        "POST",
        "/v1/thread-operations",
        Some(
            json!({"target":{"type":"thread","thread_ref":"cmaeg1.tr.original"},"action":"rename","name":"Incident triage"}),
        ),
        200,
        unknown.clone(),
        true,
    );
    request
        .headers
        .insert("idempotency-key".into(), "rename-stable".into());
    server.expect(request);
    same(
        client
            .manage_thread("rename-stable", &rename)
            .await
            .unwrap(),
        unknown,
    );
    server.assert_drained();
}

#[path = "cmaeg/history.rs"]
mod history;

#[test]
fn quoted_message_content_and_reference_only_forms_roundtrip_without_changing_legacy_bytes() {
    let legacy = json!({"type":"message","text":"current","mentions_bot":false});
    let activity: ActivityKind = serde_json::from_value(legacy.clone()).unwrap();
    same(activity, legacy.clone());
    for quote in [
        json!({"relation":"reply","message_id":"8","content":{"text":{"text":"untrusted quote","truncated":true,"origin":"parent_text"},"kinds":["image"]}}),
        json!({"relation":"context","message_id":"wamid.parent"}),
    ] {
        let mut value = legacy.clone();
        value["quoted_message"] = quote;
        let activity: ActivityKind = serde_json::from_value(value.clone()).unwrap();
        same(activity, value);
    }
}

#[tokio::test]
async fn telegram_topic_creation_omits_archive_and_manages_the_returned_scope() {
    let server = WireServer::new().await;
    let client = client(&server);
    let examples: Value = serde_json::from_str(include_str!(
        "../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json"
    ))
    .unwrap();
    let receipt = examples["ThreadReceipt"][1].clone();
    let create = CreateThreadRequest {
        name: "Release planning".into(),
        auto_archive_duration: None,
    };
    let mut expected = wire::json(
        "POST",
        "/v1/events/original%2Fforum/threads",
        Some(json!({"name":"Release planning"})),
        200,
        receipt.clone(),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "topic-create".into());
    server.expect(expected);
    let created = client
        .create_event_thread("original/forum", "topic-create", &create)
        .await
        .unwrap();
    same(&created, receipt);
    let target = ThreadTarget::Thread {
        thread_ref: created.thread.unwrap().thread_ref,
    };
    let manage = ManageThreadRequest {
        target,
        action: ThreadAction::Close,
        name: None,
    };
    let confirmed = json!({"outcome":{"status":"confirmed"}});
    let mut expected = wire::json(
        "POST",
        "/v1/thread-operations",
        Some(
            json!({"target":{"type":"thread","thread_ref":"cmaeg1.tr.telegram-fixture"},"action":"close"}),
        ),
        200,
        confirmed.clone(),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "topic-close".into());
    server.expect(expected);
    same(
        client.manage_thread("topic-close", &manage).await.unwrap(),
        confirmed,
    );
    server.assert_drained();
}

#[test]
fn telegram_create_request_preserves_omission_and_rejects_explicit_null() {
    let value = r#"{"name":"Release planning"}"#;
    let request: CreateThreadRequest = serde_json::from_str(value).unwrap();
    assert!(request.auto_archive_duration.is_none());
    assert_eq!(serde_json::to_string(&request).unwrap(), value);
    assert!(
        serde_json::from_str::<CreateThreadRequest>(
            r#"{"name":"Release planning","auto_archive_duration":null}"#
        )
        .is_err()
    );
}
