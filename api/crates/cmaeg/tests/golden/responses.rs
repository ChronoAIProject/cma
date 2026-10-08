//! I7: for unrouted channels, management and ingress responses keep their
//! bytes. Each legacy channel is created through the public management API
//! and read back with `GET /v1/channels/{id}`; the body is compared, byte for
//! byte, with the response recorded before the routing projection existed,
//! so a legacy channel never gains a `routing` key or any other change.
//!
//! Callback responses, custom acceptance, raw Sync SSE and result webhook
//! bytes are also pinned before fan-out ingress is introduced.
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

/// Substitute only named per-run JSON strings in the original wire bytes.
/// Serializing the whole value here would hide changes in ordering or framing.
fn normalize_fields(raw: &str, value: &Value, fields: &[&str]) -> String {
    let mut normalized = raw.to_owned();
    for field in fields {
        let text = value[*field].as_str().expect("named dynamic string");
        let original = serde_json::to_string(text).unwrap();
        let replacement = serde_json::to_string(&format!("<{field}>")).unwrap();
        normalized = normalized.replace(&original, &replacement);
    }
    normalized
}

#[tokio::test]
async fn legacy_callback_responses_keep_their_bytes() {
    let mut app = GoldenApp::start().await;
    let mut source = relay("telegram");
    source["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
    let channel = app
        .create(
            json!([source]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let mut envelope = super::relay_support::Envelope::new();
    let response = app.callback(&channel, &envelope).await;
    assert_eq!(response.status(), 202);
    let raw = response.text().await.unwrap();
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["status"], "accepted");
    assert_golden(
        "responses/callback_accepted.json",
        &format!("{}\n", normalize_fields(&raw, &value, &["conversation_id"])),
    );
    app.quiet().await;
    let count = app.captured().len();
    envelope.group();
    let response = app.callback(&channel, &envelope).await;
    assert_eq!(response.status(), 202);
    let raw = response.text().await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap()["reason"],
        "group_not_addressed"
    );
    assert_golden("responses/callback_refused.json", &format!("{raw}\n"));
    assert_eq!(app.captured().len(), count, "refusal launches nothing");
    app.drain().await;
}

#[tokio::test]
async fn legacy_event_acceptance_and_stream_keep_their_bytes() {
    let mut app = GoldenApp::start().await;
    let channel = app
        .create(json!([custom()]), "per_event", json!([{"type":"sync"}]))
        .await;
    let event = |id: &str| {
        json!({"specversion":"1.0","id":id,"type":"order.created",
        "source":"https://orders.example","data":{"order":1}})
    };
    let response = app.event(&channel, "", &event("accepted-1")).await;
    assert_eq!(response.status(), 202);
    let raw = response.text().await.unwrap();
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["status"], "accepted");
    assert_golden(
        "responses/event_accepted.json",
        &format!("{}\n", normalize_fields(&raw, &value, &["conversation_id"])),
    );
    app.quiet().await;
    let response = app
        .event(&channel, "?mode=stream", &event("streamed-1"))
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let raw = response.text().await.unwrap();
    let first = raw
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let value: Value = serde_json::from_str(first).unwrap();
    assert_golden(
        "responses/event_stream.sse",
        &normalize_fields(&raw, &value, &["conversation_id"]),
    );
    app.drain().await;
}

#[tokio::test(start_paused = true)]
async fn legacy_result_webhook_bodies_keep_their_bytes() {
    use super::{
        fake_http::FakeHttp,
        recording_fixtures as fixtures,
        recording_support::{Hooks, Recording},
    };
    use cmaeg::turns::outcomes::TurnOutcome;
    use cmaeg_core::{activity::SourceKind, seal::ReplyAddress};

    for (name, terminal) in [
        (
            "completed",
            TurnOutcome::Completed {
                final_text: Some("The answer.".into()),
            },
        ),
        (
            "failed",
            TurnOutcome::Failed {
                code: "provider_failed".into(),
                message: "provider detail".into(),
            },
        ),
    ] {
        let channel = fixtures::custom_channel();
        let activity = fixtures::activity(SourceKind::Custom, "event-1");
        let event = fixtures::event(&channel, &activity, 1, ReplyAddress::None);
        let hooks = Hooks::answering([]);
        let recording = Recording::start(FakeHttp::platforms(), hooks.clone(), Default::default());
        let turn = fixtures::turn(&recording.keys, &channel, activity, &event);
        recording
            .outcome(
                &turn,
                TurnOutcome::Accepted {
                    response_id: "response-1".into(),
                },
            )
            .await;
        recording.outcome(&turn, terminal).await;
        recording.finish().await;
        let bodies = hooks.raw_bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let value: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(value["type"], format!("cmaeg.response.{name}"));
        assert_golden(
            &format!("responses/result_webhook_{name}.json"),
            &format!(
                "{}\n",
                normalize_fields(&bodies[0], &value, &["id", "time"])
            ),
        );
    }
}
