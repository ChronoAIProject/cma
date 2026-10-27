//! I7: for unrouted events on legacy channels, the context tools keep their
//! response bytes. Each scenario creates a legacy channel through the public
//! management API, drives one event through ingress, and reads the event's
//! context with `readEventContext` and attests it with `verifyEventContext`
//! under the channel's agent key. Both bodies are compared, byte for byte,
//! with the responses recorded before tool calls were bound to consumers, so
//! a legacy event never gains a `consumer` key. Native Telegram explicitly
//! asserts and strips only its additive native-media capability before comparison.
//!
//! Updating these fixtures is a reviewed contract change; see `fixture.rs`.
use super::{
    app::{agent_key, GoldenApp, ISSUER},
    fixture::assert_golden,
    relay_support::{now, Envelope},
};
use serde_json::{json, Value};

/// The event context of the scenario's only turn, as the provider received it.
fn turn_context(app: &GoldenApp) -> Value {
    let turns: Vec<Value> = app
        .captured()
        .iter()
        .filter(|call| call.path == "/api/v2/responses")
        .map(|call| serde_json::from_str(&call.body).unwrap())
        .collect();
    assert_eq!(turns.len(), 1, "exactly one turn");
    turns[0]["event_context"].clone()
}

/// Replace the values a run cannot reproduce, each exact value by name, in
/// the raw body, so key order and every other byte stay pinned.
fn normalize(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap();
    let mut text = body.to_owned();
    for (pointer, placeholder) in [
        ("/channel_id", "<channel_id>"),
        ("/provider_binding_id", "<binding_id>"),
        ("/conversation_id", "<conversation_id>"),
        ("/activity/occurred_at", "<occurred_at>"),
    ] {
        let raw = value.pointer(pointer).and_then(Value::as_str).unwrap();
        text = text.replace(raw, placeholder);
    }
    let expires_at = value["expires_at"].as_i64().unwrap();
    text = text.replace(
        &format!("\"expires_at\":{expires_at}"),
        "\"expires_at\":\"<expires_at>\"",
    );
    text.push('\n');
    text
}

/// Read and verify the context of the scenario's event and pin both bodies.
async fn pin(app: &GoldenApp, name: &str) {
    let context = turn_context(app);
    let reference = context["event_ref"].as_str().unwrap();
    let url = format!("{}/v1/events/{reference}/context", app.base);
    let read = app
        .client
        .get(&url)
        .bearer_auth(agent_key())
        .send()
        .await
        .unwrap();
    let status = read.status();
    let read = read.text().await.unwrap();
    assert_eq!(status, 200, "{name}: {read}");
    let verify = app
        .client
        .post(&url)
        .bearer_auth(agent_key())
        .json(&context["activity"])
        .send()
        .await
        .unwrap();
    let status = verify.status();
    let verify = verify.text().await.unwrap();
    assert_eq!(status, 200, "{name}: {verify}");
    for body in [&read, &verify] {
        assert!(!body.contains("\"consumer\""), "{name}: {body}");
        assert!(!body.contains("\"routing\""), "{name}: {body}");
    }
    let retained = |body: String| {
        if name != "native_telegram" {
            return body;
        }
        let value: Value = serde_json::from_str(&body).unwrap();
        let capability = &value["capabilities"]["native_media"];
        assert_eq!(
            capability,
            &json!({"kinds":["image","file","audio","video"],"sources":["url","base64"],"max_attachments":4,"max_inline_bytes":65536})
        );
        // Remove only the reviewed additive field; keep all historical response bytes pinned.
        // Value serialization sorts object keys, while the wire follows struct order.
        let expected = ",\"native_media\":{\"kinds\":[\"image\",\"file\",\"audio\",\"video\"],\"sources\":[\"url\",\"base64\"],\"max_attachments\":4,\"max_inline_bytes\":65536}";
        assert_eq!(body.matches(expected).count(), 1);
        body.replacen(expected, "", 1)
    };
    assert_golden(
        &format!("tools/{name}_read.json"),
        &normalize(&retained(read)),
    );
    assert_golden(
        &format!("tools/{name}_verify.json"),
        &normalize(&retained(verify)),
    );
}

#[tokio::test]
async fn legacy_relay_context_responses_keep_their_bytes() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([{"type":"nyxid_relay","issuer":ISSUER,"key_id":"key-1","route_ids":["route-1"],
                "platform":"telegram","bot_username":"golden_bot","admission":{"type":"all"}}]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let response = app.callback(&channel, &Envelope::new()).await;
    assert_eq!(response.status(), 202);
    app.quiet().await;
    pin(&app, "relay").await;
    app.drain().await;
}

#[tokio::test]
async fn native_telegram_context_adds_only_reviewed_media_capability() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([{"type":"telegram","bot_id":"77","bot_username":"golden_bot","admission":{"type":"all"}}]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let update = json!({"update_id":101,"message":{"message_id":5,"date":now()-5,
        "chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}});
    assert_eq!(app.telegram(&channel, &update).await.status(), 202);
    app.quiet().await;
    pin(&app, "native_telegram").await;
    app.drain().await;
}

#[tokio::test]
async fn legacy_custom_context_responses_keep_their_bytes() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}]),
            "per_event",
            json!([{"type":"source"}]),
        )
        .await;
    let event = json!({"specversion":"1.0","id":"event-1","type":"order.created",
        "source":"https://orders.example","subject":"customer-1","data":{"z":1,"a":2}});
    assert_eq!(app.event(&channel, "", &event).await.status(), 202);
    app.quiet().await;
    pin(&app, "custom").await;
    app.drain().await;
}
