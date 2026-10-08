//! Authenticated CloudEvent targets and transport-specific refusals.
use super::{
    routing_fixture::{self, NYX, REVIEWER},
    support::App,
};
use cmaeg_core::{activity::ActivityKind, seal::event_ref::activity_digest};
use serde_json::{json, Value};

pub async fn build_app(enabled: bool, edit: impl FnOnce(&mut Value)) -> App {
    App::new(enabled, |raw| {
        raw["sources"]
            .as_array_mut()
            .unwrap()
            .push(routing_fixture::custom());
        raw["reply"]["sinks"] = json!([{"type":"sync"}]);
        edit(raw);
    })
    .await
}
pub fn event(targets: Option<&str>) -> Value {
    let mut value = json!({"specversion":"1.0","id":"custom-event-1","type":"order.created","source":"https://orders.example","data":{"message":"private-message"}});
    if let Some(targets) = targets {
        value["cmaegtargets"] = targets.into();
    }
    value
}
pub async fn problem(response: reqwest::Response, status: u16, detail: &str) -> Value {
    assert_eq!(response.status().as_u16(), status);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], detail, "{body}");
    assert_eq!(
        body["code"],
        if status == 400 {
            "invalid_request"
        } else {
            "provider_unavailable"
        }
    );
    body
}

#[tokio::test]
async fn structured_targets_bind_one_activity_and_distinct_child_references() {
    let app = build_app(true, |_| {}).await;
    let response = app.custom(event(Some("reviewer,main")), false).await;
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["consumers"][0]["consumer_id"], REVIEWER);
    assert_eq!(body["consumers"][1]["consumer_id"], "default");
    app.idle().await;
    let turns = app.recorder.requests.lock().unwrap();
    assert_eq!(turns.len(), 2);
    assert_ne!(turns[0].conversation_id, turns[1].conversation_id);
    for turn in turns.iter() {
        let payload = turn
            .event_ref
            .open(&app.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        assert_eq!(
            payload.activity_sha256,
            Some(activity_digest(&turn.activity))
        );
        assert_eq!(payload.consumer, turn.consumer);
        assert_eq!(
            serde_json::to_value(&turn.activity).unwrap(),
            serde_json::to_value(&turns[0].activity).unwrap()
        );
    }
    assert_eq!(app.observer.routed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn binary_hmac_targets_are_unsigned_but_bearer_targets_are_authenticated() {
    let app = build_app(true, |_| {}).await;
    let binary = |request: reqwest::RequestBuilder| {
        request
            .header("ce-specversion", "1.0")
            .header("ce-id", "binary-1")
            .header("ce-source", "https://orders.example")
            .header("ce-type", "order.created")
    };
    let response = binary(app.custom_request(json!({}).to_string(), false, false))
        .header("ce-cmaegtargets", "reviewer")
        .send()
        .await
        .unwrap();
    problem(response, 400, "targets_unsigned").await;
    let response = binary(app.custom_request(json!({}).to_string(), false, false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap()["consumers"][0]["consumer_id"],
        "default"
    );
    let bearer = build_app(true, |raw| {
        raw["sources"][3]["auth"] =
            json!({"type":"nyxid_key","key_hashes":[routing_fixture::hash("fixture-event-key")]});
    })
    .await;
    let response = binary(bearer.custom_request(json!({}).to_string(), false, false))
        .header("authorization", "Bearer fixture-event-key")
        .header("ce-cmaegtargets", "reviewer")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap()["consumers"][0]["consumer_id"],
        REVIEWER
    );
}

#[tokio::test]
async fn custom_target_errors_are_precise_and_never_send_chat_notices() {
    let mut app = build_app(true, |raw| raw["routing"]["max_targets"] = 2.into()).await;
    for (value, detail) in [
        (event(Some("ghost")), "target_unknown:ghost"),
        (event(Some("main,reviewer,nyx")), "too_many_targets"),
        (event(Some("")), "invalid_targets"),
        (event(Some("nyx")), "target_ignores_actor_kind:nyx"),
    ] {
        problem(app.custom(value, false).await, 400, detail).await;
    }
    let mut invalid = event(None);
    invalid["cmaegtargets"] = json!(["reviewer"]);
    problem(app.custom(invalid, false).await, 400, "invalid_targets").await;
    assert!(app.recorder.requests.lock().unwrap().is_empty());
    assert!(app.outcomes.as_mut().unwrap().try_recv().is_err());
    let forbidden = build_app(true, |raw| {
        raw["routing"]["addressing"] = json!({"event_field":false})
    })
    .await;
    problem(
        forbidden.custom(event(Some("reviewer")), false).await,
        400,
        "targets_not_permitted",
    )
    .await;
    let ignores = build_app(true, |raw| raw["routing"]["default_targets"] = json!([NYX])).await;
    problem(
        ignores.custom(event(None), false).await,
        400,
        "no_target_accepts_actor_kind",
    )
    .await;
}

#[tokio::test]
async fn nonhuman_rules_skip_unsupported_consumers_but_explicit_targets_refuse() {
    let app = build_app(true, |raw| {
        raw["routing"]["rules"] = json!([{"id":"all","when":{},"targets":[NYX,REVIEWER]}]);
    })
    .await;
    let response = app.custom(event(None), false).await;
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["consumers"].as_array().unwrap().len(), 1);
    assert_eq!(body["consumers"][0]["consumer_id"], REVIEWER);
}

#[tokio::test]
async fn kill_switch_refuses_explicit_targets_before_sync_setup_and_preserves_legacy() {
    for missing_sync in [false, true] {
        let app = build_app(false, |raw| {
            raw["routing"]["rules"] =
                json!([{"id":"all","when":{},"targets":["default",REVIEWER]}]);
            if missing_sync {
                raw["reply"]["sinks"] = json!([{"type":"source"}]);
            }
        })
        .await;
        for stream in [false, true] {
            let response = app.custom(event(Some("reviewer")), stream).await;
            assert_eq!(response.headers()["retry-after"], "300");
            problem(response, 503, "consumer_unavailable").await;
            assert!(app.waiters.is_empty());
        }
        for targets in [None, Some("main")] {
            let response = app.custom(event(targets), false).await;
            assert_eq!(response.status(), 202);
            assert!(response
                .json::<Value>()
                .await
                .unwrap()
                .get("consumers")
                .is_none());
            let request = app.recorder.requests.lock().unwrap().pop().unwrap();
            assert!(request.activity.routing.is_none());
            assert!(
                matches!(request.activity.kind,ActivityKind::Event{ce,..} if ce.extensions.get("cmaegtargets").and_then(Value::as_str)==targets)
            );
        }
        let waiters = (0..4096)
            .map(|_| app.waiters.register().unwrap().1)
            .collect::<Vec<_>>();
        assert!(app.waiters.register().is_none());
        let response = app.custom(event(Some("reviewer")), true).await;
        assert_eq!(response.headers()["retry-after"], "300");
        problem(response, 503, "consumer_unavailable").await;
        assert_eq!(app.waiters.len(), 4096);
        drop(waiters);
    }
}

#[tokio::test]
async fn custom_capacity_is_retryable_and_drops_its_stream_waiter() {
    let app = build_app(true, |_| {}).await;
    let leases = (0..4)
        .map(|_| {
            app.capacity
                .try_lease(
                    &app.channel.pointer.record_id,
                    &app.channel.definition.owner.subject,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    for stream in [false, true] {
        let response = app.custom(event(Some("reviewer")), stream).await;
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["retry-after"], "2");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "provider_busy");
        assert_eq!(body["detail"], "channel_capacity");
        assert!(app.waiters.is_empty());
    }
    assert!(app.recorder.requests.lock().unwrap().is_empty());
    drop(leases);
}

#[tokio::test]
async fn all_consumers_unavailable_maps_to503() {
    let app = build_app(true, |raw| {
        raw["routing"]["consumers"][0]["pending"] = true.into();
        raw["routing"]["rules"] = json!([{"id":"all","when":{},"targets":[REVIEWER]}]);
    })
    .await;
    problem(
        app.custom(event(None), false).await,
        503,
        "consumer_unavailable",
    )
    .await;
    assert!(app.recorder.requests.lock().unwrap().is_empty());
}
