//! I5: for unrouted activities on legacy channels, provider calls are
//! byte-identical. Each scenario creates its channels through the public
//! management API, drives one ingress or tool path through the composed
//! gateway, and compares every provider call it made — the binding body, the
//! Responses body with its metadata and event context, conversation and
//! event-context requests — with the fixture recorded before the change.
//!
//! Updating these fixtures is a reviewed contract change; see `fixture.rs`.
use super::{
    app::{agent_key, GoldenApp, ISSUER},
    fake_provider::ReceivedRequest,
    fixture::assert_golden,
    provider_normalize::Normalizer,
    relay_support::{now, Envelope},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn relay(platform: &str, identity: Value) -> Value {
    let mut source = json!({"type":"nyxid_relay","issuer":ISSUER,"key_id":"key-1",
        "route_ids":["route-1"],"platform":platform,"admission":{"type":"all"}});
    for (key, value) in identity.as_object().unwrap() {
        source[key] = value.clone();
    }
    source
}

fn native(bot_username: &str) -> Value {
    json!({"type":"telegram","bot_id":"77","bot_username":bot_username,"admission":{"type":"all"}})
}

/// A Telegram relay message in a group whose text addresses `username`.
fn addressed(username: &str) -> Envelope {
    let text = format!("@{username} hello");
    let mut envelope = Envelope::new();
    envelope.group();
    envelope.payload["content"]["text"] = text.clone().into();
    let message = &mut envelope.payload["raw_platform_data"]["message"];
    message["text"] = text.into();
    message["entities"] = json!([{"type":"mention","offset":0,"length":username.len() + 1}]);
    envelope
}

async fn accepted(response: reqwest::Response) {
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "accepted", "{body}");
}

/// The Responses body of the scenario's only turn.
fn turn_body(calls: &[ReceivedRequest]) -> Value {
    let turns: Vec<_> = calls
        .iter()
        .filter(|call| call.path == "/api/v2/responses")
        .collect();
    assert_eq!(turns.len(), 1, "exactly one turn");
    serde_json::from_str(&turns[0].body).unwrap()
}

#[tokio::test]
async fn relay_telegram_mention_strips_the_bot_address() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([relay("telegram", json!({"bot_username":"support_bot"}))]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    accepted(app.callback(&channel, &addressed("support_bot")).await).await;
    app.quiet().await;
    let calls = app.captured();
    let input = &turn_body(&calls)["input"][0]["content"][0]["text"];
    assert_eq!(input, "hello");
    assert_golden(
        "provider/relay_telegram_mention.json",
        &Normalizer::fixture(&calls),
    );
    app.drain().await;
}

#[tokio::test]
async fn native_telegram_message() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([native("golden_bot")]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let update = json!({"update_id":101,"message":{"message_id":5,"date":now()-5,
        "chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}});
    accepted(app.telegram(&channel, &update).await).await;
    app.quiet().await;
    assert_golden(
        "provider/native_telegram.json",
        &Normalizer::fixture(&app.captured()),
    );
    app.drain().await;
}

#[tokio::test]
async fn custom_cloud_event_accepted_and_streamed() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}]),
            "per_event",
            json!([{"type":"sync"}]),
        )
        .await;
    let event = |id: &str| {
        json!({"specversion":"1.0","id":id,"type":"order.created","source":"https://orders.example",
            "subject":"customer-1","data":{"z":1,"a":2}})
    };
    let response = app.event(&channel, "", &event("event-1")).await;
    assert_eq!(response.status(), 202);
    app.quiet().await;
    let response = app.event(&channel, "?mode=stream", &event("event-2")).await;
    assert_eq!(response.status(), 200);
    let frames = response.text().await.unwrap();
    assert!(frames.contains("hello"), "{frames}");
    app.quiet().await;
    assert_golden(
        "provider/custom_cloud_event.json",
        &Normalizer::fixture(&app.captured()),
    );
    app.drain().await;
}

#[tokio::test]
async fn lark_mention_keeps_the_mention_key() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([relay("lark", json!({"bot_id":"ou_bot"}))]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    envelope.lark_text("@_user_1 status?");
    envelope.lark_message()["mentions"] = json!([{"key":"@_user_1","id":{"open_id":"ou_bot","union_id":"on_bot"},"name":"Support Bot"}]);
    accepted(app.callback(&channel, &envelope).await).await;
    app.quiet().await;
    assert_golden(
        "provider/lark_mention.json",
        &Normalizer::fixture(&app.captured()),
    );
    app.drain().await;
}

/// G32: the renderer strips the address of the first source that declares a
/// bot username, whichever bot a message addresses. A native and a relay
/// Telegram source with different usernames therefore keep the second bot's
/// address in the text. The golden pins that behaviour.
#[tokio::test]
async fn native_and_relay_telegram_strip_only_the_first_source_username() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([
                native("first_bot"),
                relay("telegram", json!({"bot_username":"second_bot"}))
            ]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    accepted(app.callback(&channel, &addressed("second_bot")).await).await;
    app.quiet().await;
    let calls = app.captured();
    let input = &turn_body(&calls)["input"][0]["content"][0]["text"];
    assert_eq!(input, "@second_bot hello");
    assert_golden(
        "provider/native_and_relay_telegram.json",
        &Normalizer::fixture(&calls),
    );
    app.drain().await;
}

/// `readEventContext` reads the provider's event context; `verifyEventContext`
/// attests a supplied activity without any provider call.
#[tokio::test]
async fn event_context_tools() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([relay("telegram", json!({"bot_username":"support_bot"}))]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    accepted(app.callback(&channel, &Envelope::new()).await).await;
    app.quiet().await;
    let turn = turn_body(&app.captured());
    // A relay reference exceeds the metadata value bound, so the turn carries
    // it only in its event context.
    let reference = turn["event_context"]["event_ref"].as_str().unwrap();
    let url = format!("{}/v1/events/{reference}/context", app.base);
    let read = app
        .client
        .get(&url)
        .bearer_auth(agent_key())
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    // A legacy relay event reads back verified, with no routing decision.
    let context: Value = read.json().await.unwrap();
    assert_eq!(context["activity_verified"], true);
    assert!(context["activity"].get("routing").is_none());
    let verified = app
        .client
        .post(&url)
        .bearer_auth(agent_key())
        .json(&turn["event_context"]["activity"])
        .send()
        .await
        .unwrap();
    assert_eq!(verified.status(), 200, "{}", verified.text().await.unwrap());
    assert_golden(
        "provider/event_context_tools.json",
        &Normalizer::fixture(&app.captured()),
    );
    app.drain().await;
}

#[tokio::test]
async fn test_channel_turn() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(
            json!([relay("telegram", json!({"bot_username":"support_bot"}))]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let response = app
        .client
        .post(format!("{}/v1/channels/{channel}/test", app.base))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["first_text"], "hello");
    assert_golden(
        "provider/test_channel.json",
        &Normalizer::fixture(&app.captured()),
    );
    app.drain().await;
}

#[test]
fn normalizer_replaces_only_named_values_and_hashes_credentials() {
    let call = |method: &str, path: &str, key: Option<&str>, body: &str| ReceivedRequest {
        received_at: std::time::Instant::now(),
        method: method.into(),
        path: path.into(),
        authorization: Some("Bearer nyx_example".into()),
        idempotency_key: key.map(str::to_owned),
        content_type: Some("application/json".into()),
        body: body.into(),
    };
    let calls = [
        call(
            "PUT",
            "/api/v1/cmaeg/bindings/bnd_1/conversations/conv_a",
            Some(&format!("test_{}", "ab".repeat(16))),
            r#"{"agent_key":"nyx_example","channel_id":"kept-shape","text":"<media-0123456789ab> conv_a"}"#,
        ),
        call(
            "POST",
            "/api/v2/responses",
            Some("evt_1"),
            r#"{"conversation":"conv_a","activity":{"conversation":{"id":"chat"},"occurred_at":"t"}}"#,
        ),
    ];
    let fixture: Value = serde_json::from_str(&Normalizer::fixture(&calls)).unwrap();
    let hash = |value: &str| format!("sha256:{:x}", Sha256::digest(value.as_bytes()));
    assert_eq!(
        fixture[0]["path"],
        "/api/v1/cmaeg/bindings/<binding_id:1>/conversations/<conversation_id:1>"
    );
    assert_eq!(fixture[0]["idempotency_key"], "test_<test_turn:1>");
    assert_eq!(fixture[0]["authorization"], hash("Bearer nyx_example"));
    let first: Value = serde_json::from_str(fixture[0]["body"].as_str().unwrap()).unwrap();
    assert_eq!(first["agent_key"], hash("nyx_example"));
    assert_eq!(first["channel_id"], "<channel_id:1>");
    // Unnamed values keep their bytes, even when they repeat a named value.
    assert_eq!(first["text"], "<media-<fence_nonce:1>> conv_a");
    assert_eq!(fixture[1]["idempotency_key"], "evt_1");
    assert_eq!(
        fixture[1]["body"],
        r#"{"conversation":"<conversation_id:1>","activity":{"conversation":{"id":"chat"},"occurred_at":"<timestamp:1>"}}"#
    );
}
