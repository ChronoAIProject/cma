use super::support::{problem, App};
use cmaeg::{auth::AgentCaller, channels::resolver::ChannelResolver, records::RecordClient};
use cmaeg_core::seal::EventRef;
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
#[tokio::test]
async fn replies_await_engine_receipt_enforce_key_and_ref_auth_and_mark_dedupe() {
    let app = App::new().await;
    let url = app.event_url("replies");
    let client = &app.fixture.client;
    problem(
        client
            .post(&url)
            .json(&json!({"text":"hello"}))
            .send()
            .await
            .unwrap(),
        401,
        "not_authenticated",
    )
    .await;
    let wrong = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    problem(
        client
            .post(&url)
            .bearer_auth(wrong)
            .json(&json!({"text":"hello"}))
            .send()
            .await
            .unwrap(),
        403,
        "forbidden",
    )
    .await;
    let response = client
        .post(&url)
        .bearer_auth(&app.key)
        .json(&json!({"text":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["message_ref"], app.message_ref.as_str());
    assert_eq!(receipt["delivery"]["outcome"], "confirmed");
    assert_eq!(receipt["delivery"]["platform_message_id"], "platform-1");
    assert_eq!(app.requests.lock().unwrap().len(), 1);
    assert!(app
        .state
        .tracker
        .has_tool_reply(app.event_ref.as_str(), super::relay_support::now()));
    let tampered = format!("{}invalid", app.event_ref.as_str());
    problem(
        client
            .post(format!("{}/v1/events/{tampered}/replies", app.base))
            .bearer_auth(&app.key)
            .json(&json!({"text":"hello"}))
            .send()
            .await
            .unwrap(),
        400,
        "event_ref_invalid",
    )
    .await;
    let mut expired = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    expired.expires_at = super::relay_support::now() - 1;
    let expired = EventRef::seal(&expired, &app.state.keys).unwrap();
    problem(
        client
            .post(format!(
                "{}/v1/events/{}/replies",
                app.base,
                expired.as_str()
            ))
            .bearer_auth(&app.key)
            .json(&json!({"text":"hello"}))
            .send()
            .await
            .unwrap(),
        410,
        "event_ref_expired",
    )
    .await;
}
#[tokio::test]
async fn context_reads_full_provider_activity_after_a_cold_replica_restore_without_secrets() {
    let text = "🦀".repeat(8192);
    let app = App::with_text(&text).await;
    let response = app
        .fixture
        .client
        .get(app.event_url("context"))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let raw = response.text().await.unwrap();
    assert!(!raw.contains(&app.key));
    assert!(!raw.contains("reply_token"));
    assert!(!raw.contains(app.event_ref.as_str()));
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["event_id"], "message-1");
    assert_eq!(value["activity_verified"], true);
    assert_eq!(value["source"]["type"], "nyxid_relay");
    assert_eq!(value["channel_id"], app.fixture.id.as_str());
    assert_eq!(value["activity"]["kind"]["text"], text);
    assert_eq!(value["activity"]["actor"]["id"], "7");
    assert_eq!(value["capabilities"]["typing"], false);
    assert!(value["activity"].get("source").is_none());
    // An unrouted activity projects exactly as before: no routing key.
    assert!(value["activity"].get("routing").is_none());
    let resolved = app
        .state
        .resolver
        .resolve(app.fixture.id.as_str())
        .await
        .unwrap();
    let origin = reqwest::Url::parse(&resolved.pointer.record_url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let mut cold = app.state.clone();
    cold.resolver = ChannelResolver::new(
        Arc::new(RecordClient::new(&origin, "test-service-bearer").unwrap()),
        cold.keys.clone(),
        Default::default(),
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    let context = cold
        .context(AgentCaller::from_headers(&headers).unwrap(), &app.event_ref)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(context).unwrap(), value);
    assert_eq!(app.fixture.backend.records.load(Ordering::SeqCst), 2);
    assert_eq!(
        app.fake
            .requests()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}
#[tokio::test]
async fn context_reports_each_relay_platform_and_its_capabilities() {
    for (platform, capabilities) in [
        (
            "telegram",
            json!({"edit":true,"typing":false,"buttons":"inline_keyboard","cards":"text","attachments":"link_text","max_text_chars":4000}),
        ),
        (
            "lark",
            json!({"edit":false,"typing":false,"buttons":"numbered_text","cards":"text","attachments":"link_text","max_text_chars":4000}),
        ),
    ] {
        let app = App::on(platform).await;
        let response = app
            .fixture
            .client
            .get(app.event_url("context"))
            .bearer_auth(&app.key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["source"]["platform"], platform);
        assert_eq!(value["activity"]["conversation"]["kind"], "group");
        assert_eq!(value["capabilities"], capabilities);
    }
}
#[tokio::test]
async fn deleted_channel_refuses_and_relay_update_and_typing_are_unsupported() {
    let app = App::new().await;
    let update = app
        .fixture
        .client
        .post(format!(
            "{}/v1/messages/{}/update",
            app.base, app.message_ref
        ))
        .bearer_auth(&app.key)
        .json(&json!({"text":"replace"}))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), 200);
    assert_eq!(
        update.json::<Value>().await.unwrap()["delivery"]["outcome"],
        "unsupported"
    );
    let typing = app
        .fixture
        .client
        .post(app.event_url("typing"))
        .bearer_auth(&app.key)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        typing.json::<Value>().await.unwrap()["outcome"],
        "unsupported"
    );
    app.fixture
        .backend
        .record_status
        .store(404, Ordering::SeqCst);
    app.state.resolver.invalidate(app.fixture.id.as_str());
    problem(
        app.fixture
            .client
            .post(app.event_url("replies"))
            .bearer_auth(&app.key)
            .json(&json!({"text":"hello"}))
            .send()
            .await
            .unwrap(),
        404,
        "channel_not_found",
    )
    .await;
}
#[tokio::test]
async fn twenty_kib_text_passes_forty_kib_fails_and_methods_are_explicit() {
    let app = App::new().await;
    let reply = |size| {
        app.fixture
            .client
            .post(app.event_url("replies"))
            .bearer_auth(&app.key)
            .json(&json!({"text":"x".repeat(size)}))
    };
    assert_eq!(reply(20 * 1024).send().await.unwrap().status(), 200);
    problem(
        app.fixture
            .client
            .post(app.event_url("typing"))
            .bearer_auth(&app.key)
            .send()
            .await
            .unwrap(),
        415,
        "unsupported_media_type",
    )
    .await;
    problem(
        reply(40 * 1024).send().await.unwrap(),
        413,
        "payload_too_large",
    )
    .await;
    assert_eq!(
        app.requests.lock().unwrap()[0].text.as_ref().unwrap().len(),
        20 * 1024
    );
    problem(
        app.fixture
            .client
            .delete(app.event_url("replies"))
            .bearer_auth(&app.key)
            .send()
            .await
            .unwrap(),
        405,
        "invalid_request",
    )
    .await;
    let document = app
        .fixture
        .client
        .get(format!("{}/openapi.json", app.base))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let ids: Vec<_> = document["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|v| v.as_object().unwrap().values())
        .filter(|v| v["tags"] == json!(["tools"]))
        .map(|v| v["operationId"].as_str().unwrap())
        .collect();
    for name in [
        "replyToEvent",
        "updateMessage",
        "sendTyping",
        "readEventContext",
    ] {
        assert!(ids.contains(&name));
    }
}
#[tokio::test]
async fn unconfirmed_delivery_is_bounded_by_eight_seconds() {
    let app = App::new().await;
    app.delay.store(9000, Ordering::SeqCst);
    let start = Instant::now();
    let value = problem(
        app.fixture
            .client
            .post(app.event_url("replies"))
            .bearer_auth(&app.key)
            .json(&json!({"text":"waiting"}))
            .send()
            .await
            .unwrap(),
        504,
        "internal",
    )
    .await;
    assert_eq!(value["detail"], "delivery_timeout");
    assert!(start.elapsed() >= Duration::from_secs(8));
    assert!(start.elapsed() < Duration::from_secs(9));
    assert!(!app
        .state
        .tracker
        .has_tool_reply(app.event_ref.as_str(), super::relay_support::now()));
}

#[tokio::test]
async fn attested_context_rejects_mismatch_and_legacy_context_is_explicitly_unverified() {
    let app = App::new().await;
    let mut payload = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    payload.activity_sha256 = Some("0".repeat(64));
    let invalid = EventRef::seal(&payload, &app.state.keys).unwrap();
    let request = |reference: EventRef| {
        app.fixture
            .client
            .get(format!(
                "{}/v1/events/{}/context",
                app.base,
                reference.as_str()
            ))
            .bearer_auth(&app.key)
    };
    assert_eq!(request(invalid).send().await.unwrap().status(), 503);
    payload.activity_sha256 = None;
    let legacy = EventRef::seal(&payload, &app.state.keys).unwrap();
    let response = request(legacy).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["activity_verified"],
        false
    );
}

#[tokio::test]
async fn context_projects_the_routing_decision_of_a_routed_activity() {
    let routing = json!({
        "source_id": "relay_telegram",
        "resolution": "rule",
        "rule_id": "triage",
        "targets": [{"id": "default", "name": "assistant"}, {"id": "csm_00000000000a", "name": "reviewer"}],
        "addressing_bytes": 1
    });
    let app = App::routed(serde_json::from_value(routing.clone()).unwrap()).await;
    let response = app
        .fixture
        .client
        .get(app.event_url("context"))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["activity_verified"], true);
    assert_eq!(value["activity"]["routing"], routing);
}
