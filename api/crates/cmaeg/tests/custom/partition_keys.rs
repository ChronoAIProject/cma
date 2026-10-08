//! Custom events on a chat-keyed channel: the partition's `custom_key`
//! decides their conversation, exactly as a legacy custom channel's `key`
//! would, while chat activities keep the chat key.
use super::support::App;
use cmaeg::{channels::resolver::ResolvedChannel, ingress::custom::activity::launch_request};
use cmaeg_core::{
    channel::ChannelDefinition,
    cloudevent::CloudEvent,
    ids::ConversationId,
    seal::{open_blob, ChannelPointer, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

/// A Telegram relay beside the custom source, under a chat key with
/// `custom_key` as only a writer behind the dynamic-sources flag stores it.
fn chat_keyed(value: &mut Value, custom_key: Option<&str>) {
    value["sources"].as_array_mut().unwrap().push(json!({
        "type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
        "key_id":"key-1","route_ids":["route-1"],"platform":"telegram","admission":{"type":"all"}}));
    value["partition"]["key"] = "conversation".into();
    if let Some(key) = custom_key {
        value["partition"]["custom_key"] = key.into();
    }
    value["secrets"]["channel_key"] = "partition-channel-key".into();
}

/// The provider turn the drained app received.
fn turn(app: &App) -> Value {
    let request = app
        .provider
        .requests()
        .into_iter()
        .find(|request| request.method == "POST")
        .expect("a provider turn");
    serde_json::from_str(&request.body).unwrap()
}

#[tokio::test]
async fn a_custom_key_admits_custom_events_on_a_chat_keyed_channel() {
    for (event, expected) in [
        (json!({"subject":"customer-1"}), "customer-1"),
        (json!({}), "https://orders.example"),
    ] {
        let mut app = App::new(
            |value| chat_keyed(value, Some("event_subject")),
            Default::default(),
            Default::default(),
            0,
        )
        .await;
        let mut body = app.event("event-1");
        body.as_object_mut().unwrap().remove("subject");
        for (key, value) in event.as_object().unwrap() {
            body[key] = value.clone();
        }
        let response = app
            .request(&app.url, body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 202, "{}", response.text().await.unwrap());
        app.drain().await;
        let turn = turn(&app);
        assert_eq!(
            turn["event_context"]["activity"]["conversation"]["id"],
            expected
        );
        // The same conversation a legacy event_subject channel derives.
        let legacy =
            ConversationId::derive(&app.fixture.state.master_keys, &app.fixture.id, 1, expected);
        assert_eq!(turn["conversation"], legacy.as_str());
    }
}

/// The definition the fixture stored for the app's channel.
fn stored_definition(app: &App) -> ChannelDefinition {
    let blob = app.fixture.backend.blob.lock().unwrap().clone();
    let opened = open_blob(&app.fixture.state.master_keys, &blob).unwrap();
    serde_json::from_slice(&opened.payload).unwrap()
}

/// Normalize one event for a channel whose partition is `partition`,
/// bypassing restore so an unstorable definition can be tried too.
fn launch(app: &App, partition: Value) -> Result<String, &'static str> {
    let mut definition = stored_definition(app);
    definition.partition = serde_json::from_value(partition).unwrap();
    let channel = Arc::new(ResolvedChannel {
        id: app.fixture.id.clone(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: Instant::now(),
        version: 1,
    });
    let ce: CloudEvent = serde_json::from_value(app.event("event-1")).unwrap();
    launch_request(
        channel,
        ce,
        vec![],
        ReplyAddress::None,
        &app.fixture.state.master_keys,
        Instant::now(),
        0,
    )
    .map(|request| request.conversation_id.as_str().to_owned())
}

#[tokio::test]
async fn custom_ingress_keys_events_by_the_custom_key_and_refuses_a_chat_key() {
    let app = App::new(
        |value| chat_keyed(value, Some("event_subject")),
        Default::default(),
        Default::default(),
        0,
    )
    .await;
    let legacy = launch(&app, json!({"version":1,"key":"event_subject"})).unwrap();
    assert_eq!(
        launch(
            &app,
            json!({"version":1,"key":"conversation","custom_key":"event_subject"})
        ),
        Ok(legacy)
    );
    // A chat key without a custom key can only remain on a definition no
    // writer could store; it keeps its legacy refusal.
    assert_eq!(
        launch(&app, json!({"version":1,"key":"conversation"})),
        Err("custom_partition_invalid")
    );
}
