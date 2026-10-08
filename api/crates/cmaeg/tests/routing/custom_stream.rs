//! Plain streams retain exactly one consumer while other targets use their sinks.
use super::{
    custom::{build_app, event, problem},
    routing_fixture::REVIEWER,
};
use cmaeg::ingress::launch::{LaunchOutcome, LaunchRefusal};
use cmaeg_core::{
    problem::{Problem, ProblemCode},
    seal::ReplyAddress,
};
use serde_json::{json, Value};

#[tokio::test]
async fn only_sync_fanout_is_refused_before_launch_without_allocating_a_waiter() {
    let app = build_app(true, |_| {}).await;
    problem(
        app.custom(event(Some("reviewer,main")), true).await,
        400,
        "fanout_requires_stream_all",
    )
    .await;
    assert!(app.waiters.is_empty());
    assert!(app.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_webhook_or_default_destination_allows_primary_only_plain_streams() {
    for default_destination in [false, true] {
        let app=build_app(true,|raw| {
            if default_destination {
                raw["routing"]["default_reply"]=json!({"type":"telegram_chat","source_id":"telegram","chat_id":"42"});
            } else {
                raw["reply"]["sinks"].as_array_mut().unwrap().push(json!({"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"key-1"}));
            }
        }).await;
        let response = app.custom(event(Some("reviewer,main")), true).await;
        assert_eq!(response.status(), 200);
        assert!(response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        let (waiter, conversation) = {
            let requests = app.recorder.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            let open = |i: usize| {
                requests[i]
                    .event_ref
                    .open(&app.state.master_keys, super::relay_support::now())
                    .unwrap()
                    .payload
            };
            let ReplyAddress::Sync { waiter } = open(0).reply else {
                panic!("primary needs Sync")
            };
            assert!(matches!(open(1).reply, ReplyAddress::None));
            assert_eq!(requests[0].consumer.as_ref().unwrap().as_str(), REVIEWER);
            (waiter, requests[0].conversation_id.clone())
        };
        let terminal = json!({"event_id":"custom-event-1","conversation_id":conversation,"response_id":"response-fixture"});
        assert!(app
            .waiters
            .emit(&waiter, "cmaeg.completed", &terminal, true));
        let text = response.text().await.unwrap();
        assert_eq!(text.matches("event: cmaeg.completed").count(), 1, "{text}");
        assert!(text.contains(conversation.as_str()));
        assert!(app.waiters.is_empty());
        app.idle().await;
    }
}

#[tokio::test]
async fn primary_refusal_returns_its_problem_while_a_sibling_keeps_its_admission() {
    let mut app=build_app(true,|raw| {
        raw["reply"]["sinks"].as_array_mut().unwrap().push(json!({"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"key-1"}));
    }).await;
    app.recorder.outcomes.lock().unwrap().insert(
        REVIEWER.into(),
        LaunchOutcome::Refused(LaunchRefusal::new(
            Problem::new(ProblemCode::ProviderUnavailable, 503, "primary unavailable"),
            "provider_unavailable",
        )),
    );
    problem(
        app.custom(event(Some("reviewer,main")), true).await,
        503,
        "primary unavailable",
    )
    .await;
    assert!(app.waiters.is_empty());
    app.idle().await;
    assert!(matches!(
        app.outcomes.as_mut().unwrap().try_recv(),
        Ok(cmaeg::delivery::intents::DeliveryCommand::ConsumerFailed { .. })
    ));
    let identities = {
        let requests = app.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        requests
            .iter()
            .map(|r| (r.conversation_id.clone(), r.idempotency_key.clone()))
            .collect::<Vec<_>>()
    };
    app.recorder.outcomes.lock().unwrap().remove(REVIEWER);
    let response = app.custom(event(Some("reviewer,main")), false).await;
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["consumers"][0]["status"], "accepted");
    assert_eq!(body["consumers"][1]["status"], "accepted");
    app.idle().await;
    let requests = app.recorder.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for i in 0..2 {
        assert_eq!(
            (
                &requests[i + 2].conversation_id,
                &requests[i + 2].idempotency_key
            ),
            (&identities[i].0, &identities[i].1)
        );
    }
}

#[tokio::test]
async fn a_single_target_stream_is_unchanged_and_deferred_admission_still_streams() {
    let app = build_app(true, |_| {}).await;
    app.recorder
        .outcomes
        .lock()
        .unwrap()
        .insert(REVIEWER.into(), LaunchOutcome::Deferred);
    let response = app.custom(event(Some("reviewer")), true).await;
    assert_eq!(response.status(), 200);
    let waiter = {
        let requests = app.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let payload = requests[0]
            .event_ref
            .open(&app.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        let ReplyAddress::Sync { waiter } = payload.reply else {
            panic!("primary needs Sync")
        };
        waiter
    };
    assert!(app.waiters.emit(
        &waiter,
        "cmaeg.completed",
        &json!({"event_id":"custom-event-1"}),
        true
    ));
    assert!(response.text().await.unwrap().contains("cmaeg.completed"));
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn a_seal_failure_releases_the_registered_stream_waiter_before_launch() {
    let app = build_app(true, |_| {}).await;
    let mut body = event(Some("reviewer"));
    // A valid 512-byte id expands sixfold in JSON before the seal's base64
    // envelope, exceeding the token cap after the Sync waiter is registered.
    body["id"] = "\u{1}".repeat(512).into();
    problem(app.custom(body, true).await, 400, "event_ref_too_large").await;
    assert!(app.waiters.is_empty());
    assert!(app.recorder.requests.lock().unwrap().is_empty());
}
