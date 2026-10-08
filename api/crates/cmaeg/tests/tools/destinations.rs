//! REST choice parsing and the routed context capability through real HTTP.
use super::{consumer_support::add_reviewer, media_support::MediaSetup, support::App};
use cmaeg_core::channel::ReplyChoice;
use serde_json::{json, Value};

#[tokio::test]
async fn rest_destination_preserves_legacy_queries_but_never_ignores_default() {
    for flag in [false, true] {
        let app = App::with_routing_flag(None, flag).await;
        for (query, expected, choice) in [
            ("", 200, None),
            (
                "?destination=origin",
                200,
                flag.then_some(ReplyChoice::Origin),
            ),
            (
                "?destination=default",
                if flag { 200 } else { 400 },
                Some(ReplyChoice::Default),
            ),
            ("?destination=garbage", if flag { 400 } else { 200 }, None),
            (
                "?destination=origin&destination=origin",
                if flag { 400 } else { 200 },
                None,
            ),
            ("?destination=origin&destination=%64efault", 400, None),
        ] {
            let before = app.destinations.lock().unwrap().len();
            let response = app
                .fixture
                .client
                .post(format!("{}{query}", app.event_url("replies")))
                .bearer_auth(&app.key)
                .json(&json!({"text":"choice"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "flag={flag} {query}");
            let value: Value = response.json().await.unwrap();
            let destinations = app.destinations.lock().unwrap();
            if expected == 200 {
                assert_eq!(destinations.len(), before + 1);
                assert_eq!(destinations.last(), Some(&choice));
            } else {
                assert_eq!(
                    destinations.len(),
                    before,
                    "invalid choice must not be queued"
                );
                assert_eq!(value["code"], "invalid_request");
                if !flag {
                    assert_eq!(value["detail"], "multi_consumer_disabled");
                }
            }
        }
    }
}

fn with_default(value: &mut Value) {
    add_reviewer(value);
    value["reply"]["sinks"] = json!([{"type":"result_webhook", "url":"https://receiver.example/events", "hmac_key_id":"result"}]);
    value["secrets"]["hmac_secrets"] = json!([["result", "result-fixture"]]);
    value["routing"]["default_reply"] = json!({"type":"sinks"});
}

#[tokio::test]
async fn default_capability_is_absent_on_legacy_and_matches_routed_availability() {
    for flag in [false, true] {
        for (edit, expected) in [
            (None, None),
            (Some(add_reviewer as fn(&mut Value)), Some(false)),
            (Some(with_default as fn(&mut Value)), Some(flag)),
        ] {
            let setup = edit.map(|edit| MediaSetup::new(vec![]).edit(edit));
            let app = App::with_routing_flag(setup, flag).await;
            let response = app
                .fixture
                .client
                .get(app.event_url("context"))
                .bearer_auth(&app.key)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let context: Value = response.json().await.unwrap();
            assert_eq!(
                context["capabilities"].get("default_destination"),
                expected.map(Value::Bool).as_ref()
            );
        }
    }
}
