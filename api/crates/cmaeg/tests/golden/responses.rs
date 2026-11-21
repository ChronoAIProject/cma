//! I7: for unrouted channels, the management `ChannelResponse` keeps its
//! bytes. Each legacy channel is created through the public management API
//! and read back with `GET /v1/channels/{id}`; the body is compared, byte for
//! byte, with the response recorded before the routing projection existed,
//! so a legacy channel never gains a `routing` key or any other change.
//!
//! Updating these fixtures is a reviewed contract change; see `fixture.rs`.
use super::{
    app::{GoldenApp, ISSUER},
    fixture::assert_golden,
};
use serde_json::{json, Value};

fn relay(platform: &str) -> Value {
    json!({"type":"nyxid_relay","issuer":ISSUER,"key_id":"key-1","route_ids":["route-1"],
        "platform":platform,"bot_username":"golden_bot","admission":{"type":"all"}})
}

fn native() -> Value {
    json!({"type":"telegram","bot_id":"77","bot_username":"golden_bot","admission":{"type":"all"}})
}

fn custom() -> Value {
    json!({"type":"custom","auth":{"type":"hmac","key_id":"ingress"}})
}

/// Replace the values a run cannot reproduce, each exact string by name, in
/// the raw body, so key order and every other byte stay pinned.
fn normalize(body: &str, channel: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap();
    let definition = &value["definition"];
    let mut replacements = vec![
        (channel.to_owned(), "<channel_id>"),
        (
            value["record_id"].as_str().unwrap().to_owned(),
            "<record_id>",
        ),
        (
            definition["created_at"].as_str().unwrap().to_owned(),
            "<created_at>",
        ),
        (
            value["provider"]["binding_id"].as_str().unwrap().to_owned(),
            "<binding_id>",
        ),
    ];
    // The golden app relocates relay JWKS to its fake NyxID on a random port.
    for source in definition["sources"].as_array().unwrap() {
        if let Some(url) = source["jwks_url"].as_str() {
            replacements.push((url.to_owned(), "<jwks_url>"));
        }
    }
    let mut text = body.to_owned();
    for (value, placeholder) in replacements {
        text = text.replace(&value, placeholder);
    }
    text.push('\n');
    text
}

#[tokio::test]
async fn legacy_channel_responses_keep_their_bytes() {
    let app = GoldenApp::start().await;
    let scenarios = [
        (
            "custom",
            json!([custom()]),
            "per_event",
            json!([{"type":"source"}]),
        ),
        (
            "native_relay_custom",
            json!([native(), relay("lark"), custom()]),
            "fixed",
            json!([{"type":"source"},{"type":"sync"}]),
        ),
        (
            "relay_telegram",
            json!([relay("telegram")]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        ),
    ];
    for (name, sources, partition, sinks) in scenarios {
        let channel = app.create(sources, partition, sinks).await;
        let response = app
            .client
            .get(format!("{}/v1/channels/{channel}", app.base))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, 200, "{name}: {body}");
        assert!(!body.contains("\"routing\""), "{name}: {body}");
        assert_golden(
            &format!("responses/{name}.json"),
            &normalize(&body, &channel),
        );
    }
}
