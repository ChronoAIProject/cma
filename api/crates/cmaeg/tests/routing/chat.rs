//! Authenticated chat ingress binds one routing decision before fan-out.
use super::{
    relay_support::Envelope,
    routing_fixture::{NYX, REVIEWER, ROUTE},
    support::*,
};
use cmaeg::ingress::launch::{LaunchOutcome, LaunchRefusal};
use cmaeg_core::{
    activity::ActivityKind,
    problem::{Problem, ProblemCode},
    seal::event_ref::activity_digest,
};
use serde_json::{json, Value};
use std::time::Duration;

async fn accepted(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "accepted", "{body}");
    body
}

#[tokio::test]
async fn relay_and_native_bind_the_same_activity_for_each_target_before_sealing() {
    let app = App::new(true, |_| {}).await;
    for native_transport in [false, true] {
        let response = if native_transport {
            app.native(&native("/to reviewer,main private-message"))
                .await
        } else {
            app.relay(&envelope("/to reviewer,main private-message"))
                .await
        };
        let body = accepted(response).await;
        app.idle().await;
        let mut requests = app.recorder.requests.lock().unwrap();
        let children = requests.drain(..).collect::<Vec<_>>();
        assert_eq!(children.len(), 2);
        assert_eq!(body["consumers"][0]["consumer_id"], REVIEWER);
        assert_eq!(body["consumers"][1]["consumer_id"], "default");
        assert_eq!(
            body["conversation_id"],
            children[0].conversation_id.as_str()
        );
        assert_eq!(
            serde_json::to_value(&children[0].activity).unwrap(),
            serde_json::to_value(&children[1].activity).unwrap()
        );
        assert_ne!(children[0].conversation_id, children[1].conversation_id);
        assert_ne!(
            children[0].event_ref.as_str(),
            children[1].event_ref.as_str()
        );
        assert_ne!(children[0].idempotency_key, children[1].idempotency_key);
        for request in children {
            let payload = request
                .event_ref
                .open(&app.state.master_keys, super::relay_support::now())
                .unwrap()
                .payload;
            assert_eq!(
                payload.activity_sha256,
                Some(activity_digest(&request.activity))
            );
            assert_eq!(payload.conversation_id, request.conversation_id);
            assert_eq!(payload.consumer, request.consumer);
            assert!(request.activity.routing.is_some());
        }
    }
    assert_eq!(app.observer.routed.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn mentions_route_on_each_relay_source_and_unknown_mentions_remain_text() {
    let app = App::new(true, |_| {}).await;
    let mut lark = Envelope::on("lark");
    lark.payload["conversation"]["id"] = ROUTE.into();
    lark.reply["conversation_id"] = ROUTE.into();
    lark.lark_text("@reviewer private-message");
    for value in [envelope("@reviewer private-message"), lark] {
        let body = accepted(app.relay(&value).await).await;
        assert_eq!(body["consumers"].as_array().unwrap().len(), 1);
        assert_eq!(body["consumers"][0]["consumer_id"], REVIEWER);
    }
    let body = accepted(app.native(&native("@stranger private-message")).await).await;
    assert_eq!(body["consumers"][0]["consumer_id"], "default");
    let requests = app.recorder.requests.lock().unwrap();
    assert!(
        matches!(&requests[2].activity.kind, ActivityKind::Message {text,..} if text == "@stranger private-message")
    );
    assert_ne!(
        requests[0].activity.source.source_id(),
        requests[1].activity.source.source_id()
    );
}

#[tokio::test]
async fn partial_refusal_and_deferred_admission_keep_per_consumer_results() {
    let mut app = App::new(true, |_| {}).await;
    app.recorder.outcomes.lock().unwrap().insert(
        REVIEWER.into(),
        LaunchOutcome::Refused(LaunchRefusal {
            problem: Problem::new(ProblemCode::ProviderUnavailable, 503, "provider refused"),
            code: "provider_rejected",
            provider_code: Some("binding_disabled".into()),
        }),
    );
    app.recorder
        .outcomes
        .lock()
        .unwrap()
        .insert("default".into(), LaunchOutcome::Deferred);
    let body = accepted(
        app.native(&native("/to reviewer,main private-message"))
            .await,
    )
    .await;
    assert_eq!(body["consumers"][0]["status"], "refused");
    assert_eq!(body["consumers"][0]["code"], "provider_rejected");
    assert_eq!(body["consumers"][0]["provider_code"], "binding_disabled");
    assert_eq!(body["consumers"][1]["status"], "deferred");
    assert_eq!(
        body["conversation_id"],
        body["consumers"][1]["conversation_id"]
    );
    app.idle().await;
    assert!(matches!(
        app.outcomes.as_mut().unwrap().try_recv(),
        Ok(cmaeg::delivery::intents::DeliveryCommand::ConsumerFailed { .. })
    ));
    assert!(app.outcomes.as_mut().unwrap().try_recv().is_err());
}

#[tokio::test]
async fn scoped_capacity_answers_retryable_503_without_affecting_legacy() {
    for owner_scope in [false, true] {
        let mut app = App::new(true, |_| {}).await;
        let mut leases = Vec::new();
        for i in 0..if owner_scope { 16 } else { 4 } {
            let record = if owner_scope {
                uuid::Uuid::new_v4().to_string().parse().unwrap()
            } else {
                app.channel.pointer.record_id.clone()
            };
            leases.push(
                app.capacity
                    .try_lease(&record, &app.channel.definition.owner.subject)
                    .unwrap(),
            );
            assert_eq!(leases.len(), i + 1);
        }
        for response in [
            app.native(&native("@reviewer x")).await,
            app.relay(&envelope("@reviewer x")).await,
        ] {
            assert_eq!(response.status(), 503);
            assert_eq!(response.headers()["retry-after"], "2");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["code"], "provider_busy");
            assert_eq!(
                body["detail"],
                if owner_scope {
                    "owner_capacity"
                } else {
                    "channel_capacity"
                }
            );
        }
        assert!(app.recorder.requests.lock().unwrap().is_empty());
        assert!(app.outcomes.as_mut().unwrap().try_recv().is_err());
        // The same decorator and exhausted capacity service still bypass leases for legacy activities.
        let legacy = App::new(false, |_| {}).await;
        accepted(legacy.native(&native("@main x")).await).await;
        let request = legacy.recorder.requests.lock().unwrap().pop().unwrap();
        assert!(request.activity.routing.is_none());
        assert!(matches!(
            app.state.launcher.launch(request).await,
            LaunchOutcome::Accepted { .. }
        ));
        drop(leases);
        accepted(app.native(&native("@reviewer x")).await).await;
    }
}

#[tokio::test]
async fn group_admission_precedes_routing_on_both_telegram_transports() {
    let app = App::new(true, |raw| {
        raw["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
        raw["sources"][1]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
    })
    .await;
    for (native_transport, bot) in [(false, "telegram_bot"), (true, "golden_bot")] {
        for (text, expected) in [
            (format!("/to@{bot} reviewer x"), "group_not_addressed"),
            (format!("@{bot} /to reviewer x"), "accepted"),
        ] {
            let response = if native_transport {
                let mut raw = native(&text);
                raw["message"]["chat"]["type"] = "supergroup".into();
                if text.starts_with('@') {
                    raw["message"]["entities"] =
                        json!([{"type":"mention","offset":0,"length":bot.len()+1}]);
                }
                app.native(&raw).await
            } else {
                let mut value = envelope(&text);
                value.group();
                if text.starts_with('@') {
                    value.payload["raw_platform_data"]["message"]["entities"] =
                        json!([{"type":"mention","offset":0,"length":bot.len()+1}]);
                }
                app.relay(&value).await
            };
            assert_eq!(response.status(), 202);
            let body: Value = response.json().await.unwrap();
            if expected == "accepted" {
                assert_eq!(body["consumers"][0]["consumer_id"], REVIEWER, "{body}");
            } else {
                assert_eq!(body["reason"], expected, "{body}");
            }
        }
    }
    assert_eq!(app.recorder.requests.lock().unwrap().len(), 2);
    assert_eq!(app.observer.routed.lock().unwrap().len(), 2);
    assert!(app.observer.refused.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pending_targets_are_skipped_and_the_kill_switch_preserves_raw_legacy_text() {
    let app = App::new(true, |raw| {
        raw["routing"]["consumers"][0]["pending"] = true.into();
        raw["routing"]["rules"] = json!([{"id":"all","when":{},"targets":[REVIEWER,NYX]}]);
    })
    .await;
    let body = accepted(app.native(&native("hello")).await).await;
    assert_eq!(body["consumers"].as_array().unwrap().len(), 1);
    assert_eq!(body["consumers"][0]["consumer_id"], NYX);
    let mut off = App::new(false, |raw| {
        raw["routing"]["rules"] = json!([{"id":"all","when":{},"targets":["default",REVIEWER]}]);
    })
    .await;
    for text in [
        "@main private-message",
        "/to main private-message",
        "private-message",
    ] {
        let body = accepted(off.native(&native(text)).await).await;
        assert!(body.get("consumers").is_none());
        let request = off.recorder.requests.lock().unwrap().pop().unwrap();
        assert!(request.activity.routing.is_none());
        assert!(
            matches!(request.activity.kind, ActivityKind::Message {text: ref raw,..} if raw == text)
        );
    }
    for text in ["@reviewer x", "/to reviewer x", "/to missing x"] {
        for response in [
            off.native(&native(text)).await,
            off.relay(&envelope(text)).await,
        ] {
            assert_eq!(response.status(), 202);
            assert_eq!(
                response.json::<Value>().await.unwrap(),
                json!({"status":"refused","reason":"consumer_unavailable"})
            );
        }
    }
    assert!(off.outcomes.as_mut().unwrap().try_recv().is_err());
}

#[tokio::test]
async fn callback_builders_reach_the_interaction_gate_without_resolving_rules() {
    let app = App::new(false, |raw| {
        raw["routing"]["default_targets"] = json!([REVIEWER]);
    })
    .await;
    let click = json!({"update_id":102,"callback_query":{"id":"click-1","data":"ic:fixture",
        "from":{"id":7,"is_bot":false},"message":{"message_id":11,"date":super::relay_support::now()-1,
        "chat":{"id":42,"type":"private"},"text":"card"}}});
    let mut relay = envelope("");
    relay.payload["raw_platform_data"] = json!({"callback_query":click["callback_query"]});
    relay.payload["content"] = json!({"type":"interaction"});
    relay.payload["reply_to_platform_message_id"] = "11".into();
    for response in [app.native(&click).await, app.relay(&relay).await] {
        assert_eq!(response.status(), 202);
        assert_eq!(
            response.json::<Value>().await.unwrap()["reason"],
            "interactive_action_unsupported"
        );
    }
    assert!(app.observer.routed.lock().unwrap().is_empty());
    assert!(app.observer.refused.lock().unwrap().is_empty());
    assert!(app.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn partial_refusal_does_not_wait_for_the_full_delivery_queue() {
    let mut app = App::new(true, |_| {}).await;
    assert_eq!(app.native(&native("/to ghost")).await.status(), 202);
    app.recorder.outcomes.lock().unwrap().insert(
        REVIEWER.into(),
        LaunchOutcome::Refused(LaunchRefusal::tracker("provider_busy")),
    );
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        app.native(&native("/to main,reviewer x")),
    )
    .await
    .unwrap();
    accepted(response).await;
    assert!(matches!(
        app.outcomes.as_mut().unwrap().recv().await,
        Some(cmaeg::delivery::intents::DeliveryCommand::Notice { .. })
    ));
    app.idle().await;
    assert!(matches!(
        app.outcomes.as_mut().unwrap().try_recv(),
        Ok(cmaeg::delivery::intents::DeliveryCommand::ConsumerFailed { .. })
    ));
}

#[tokio::test]
async fn all_provider_refusals_return_the_primary_problem_without_delivery() {
    let mut app = App::new(true, |_| {}).await;
    for id in ["default", REVIEWER] {
        app.recorder.outcomes.lock().unwrap().insert(
            id.into(),
            LaunchOutcome::Refused(LaunchRefusal::new(
                Problem::new(ProblemCode::ProviderUnavailable, 503, "provider offline"),
                "provider_unavailable",
            )),
        );
    }
    let response = app.relay(&envelope("/to reviewer,main x")).await;
    assert_eq!(response.status(), 503);
    assert!(response.headers().get("retry-after").is_none());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "provider_unavailable");
    assert_eq!(body["detail"], "provider offline");
    assert!(body.get("consumers").is_none());
    app.idle().await;
    assert!(app.outcomes.as_mut().unwrap().try_recv().is_err());
}

#[tokio::test]
async fn legacy_definitions_keep_their_response_and_raw_text_when_routing_is_on() {
    let app = App::new(true, |raw| {
        raw.as_object_mut().unwrap().remove("routing");
        raw["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("consumer_keys");
    })
    .await;
    let body = accepted(app.native(&native("/to reviewer private-message")).await).await;
    assert!(body.get("consumers").is_none());
    assert!(app.observer.routed.lock().unwrap().is_empty());
    let request = app.recorder.requests.lock().unwrap().pop().unwrap();
    assert!(request.activity.routing.is_none());
    assert!(request.lease.lock().unwrap().is_none());
    assert!(
        matches!(request.activity.kind, ActivityKind::Message {text,..} if text == "/to reviewer private-message")
    );
}
