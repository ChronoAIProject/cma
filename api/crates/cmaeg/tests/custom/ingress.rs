use super::{
    fake_provider::FakeProviderConfig,
    relay_support::probed_body,
    support::{App, Recorded},
};
use axum::extract::{Path, State};
use cmaeg::TurnConfig;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
#[tokio::test]
async fn structured_and_binary_events_admit_normalized_provider_turns_with_templates() {
    let mut app = App::new(
        |value| {
            value["sources"][0]["prompt_template"] = "{{type}} from {{source}}: {{data}}".into()
        },
        Default::default(),
        Default::default(),
        0,
    )
    .await;
    let response = app
        .request(&app.url, app.event("structured").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["event_id"], "structured");
    let response = app
        .request_media(
            &app.url,
            json!({"z":1,"a":2}).to_string(),
            "application/json",
        )
        .header("ce-specversion", "1.0")
        .header("ce-id", "binary")
        .header("ce-type", "order.created")
        .header("ce-source", "https://orders.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    app.drain().await;
    let posts: Vec<_> = app
        .provider
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST")
        .collect();
    assert_eq!(posts.len(), 2);
    for post in posts {
        let body: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(
            body["input"][0]["content"][0]["text"],
            "order.created from https://orders.example: {\"a\":2,\"z\":1}"
        );
        assert_eq!(body["event_context"]["activity"]["actor"]["kind"], "system");
        assert_eq!(
            body["event_context"]["activity"]["actor"]["id"],
            "https://orders.example"
        );
    }
    assert_eq!(app.receiver.events().len(), 2);
}
#[tokio::test]
async fn ingress_refuses_bad_auth_media_size_mode_and_duplicate_binary_attributes_without_launch() {
    let app = App::standard().await;
    let body = app.event("event").to_string();
    let response = app
        .fixture
        .client
        .post(&app.url)
        .header("content-type", "application/cloudevents+json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "not_authenticated"
    );
    assert_eq!(
        app.request(&app.url, body.clone())
            .header("webhook-timestamp", "1")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        app.request(&app.url, body.clone())
            .header("webhook-signature", "v1,invalid")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        app.request(&app.url, body.clone())
            .header("content-type", "text/plain")
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
    assert_eq!(
        app.request(&app.url, "x".repeat(262145))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    assert_eq!(
        app.request(&format!("{}?mode=bad", app.url), body)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let response = app
        .request_media(&app.url, "{}".into(), "application/json")
        .header("ce-id", "one")
        .header("ce-id", "two")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(app.provider.requests().is_empty());
    assert!(app.waiters.is_empty());
}
#[tokio::test]
async fn nyxid_key_admission_and_partition_identity_are_local_and_deterministic() {
    for (partition, expected) in [
        ("event_subject", "customer-1"),
        ("per_event", "event-1"),
        ("fixed", "fixed"),
    ] {
        let api_key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
        let mut app = App::new(|value| {
            value["sources"][0]["auth"] = json!({"type":"nyxid_key","key_hashes":[format!("sha256:{:x}",Sha256::digest(api_key.as_bytes()))]});
            value["partition"]["key"] = partition.into();
        }, Default::default(), Default::default(), 0).await;
        let body = app.event("event-1");
        assert_eq!(
            app.fixture
                .client
                .post(&app.url)
                .header("content-type", "application/cloudevents+json")
                .bearer_auth("wrong")
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            app.fixture
                .client
                .post(&app.url)
                .header("content-type", "application/cloudevents+json")
                .bearer_auth(&api_key)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            202
        );
        app.drain().await;
        let body: Value = serde_json::from_str(
            &app.provider
                .requests()
                .into_iter()
                .find(|request| request.method == "POST")
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(
            body["event_context"]["activity"]["conversation"]["id"],
            expected
        );
        assert!(body["conversation"].as_str().unwrap().starts_with("conv_"));
    }
}
#[tokio::test]
async fn acceptance_budget_defers_processing_and_source_sink_has_no_implied_webhook() {
    let mut app = App::new(
        |value| value["reply"]["sinks"] = json!([{"type":"source"}]),
        FakeProviderConfig {
            response_delay: Duration::from_millis(300),
            ..Default::default()
        },
        TurnConfig {
            accept_budget: Duration::from_millis(50),
            ..Default::default()
        },
        0,
    )
    .await;
    assert_eq!(
        app.request(
            &format!("{}?mode=stream", app.url),
            app.event("invalid-stream").to_string()
        )
        .send()
        .await
        .unwrap()
        .status(),
        400
    );
    let started = Instant::now();
    assert_eq!(
        app.request(&app.url, app.event("event-1").to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert!(started.elapsed() < Duration::from_millis(250));
    app.drain().await;
    assert!(app.receiver.events().is_empty());
}

#[tokio::test]
async fn long_source_and_subject_survive_in_activity_with_compact_reference_and_neutral_capabilities(
) {
    let mut app = App::new(
        |value| value["partition"]["key"] = "event_subject".into(),
        Default::default(),
        Default::default(),
        0,
    )
    .await;
    let mut event = app.event("event-1");
    let source = format!("https://events.example/{}", "a".repeat(3500));
    let subject = "subject".repeat(500);
    event["source"] = source.clone().into();
    event["subject"] = subject.clone().into();
    assert_eq!(
        app.request(&app.url, event.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let body: Value = serde_json::from_str(
        &app.provider
            .requests()
            .into_iter()
            .find(|request| request.method == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    let reference = body["event_context"]["event_ref"].as_str().unwrap();
    assert!(reference.len() <= 4096);
    let response = app
        .fixture
        .client
        .get(format!("{}/v1/events/{reference}/context", app.base))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let context: Value = response.json().await.unwrap();
    assert_eq!(context["activity"]["actor"]["id"], source);
    assert_eq!(context["activity"]["conversation"]["id"], subject);
    assert_eq!(context["capabilities"]["buttons"], "json");
    // A legacy custom event reads back verified, with no routing decision.
    assert_eq!(context["activity_verified"], true);
    assert!(context["activity"].get("routing").is_none());
    app.drain().await;
}
#[tokio::test]
async fn custom_event_releases_request_body_before_launch() {
    let app = Recorded::new().await;
    let signed = json!({"specversion":"1.0","id":"released","type":"order.created","source":"https://orders.example","data":{"a":1}}).to_string().into_bytes();
    let (body, dropped) = probed_body(signed.clone());
    app.fixture.recorder.expect_released(dropped.clone());
    let response = cmaeg::http::events::receive_event(
        State(app.state.clone()),
        Path(app.fixture.id.to_string()),
        app.request(&signed, body),
    )
    .await;
    assert_eq!(response.status(), 202);
    assert_eq!(app.fixture.recorder.probed.load(Ordering::SeqCst), 1);
    assert!(dropped.load(Ordering::SeqCst));
}
