//! Native forms pass through signed raw callback verification and durable CAS.
#[path = "lark_forms_execution.rs"]
mod execution;
use super::super::relay_support::Envelope;
use super::{support::Cards, *};
use cmaeg::interactions::{Destination, PreparedCard};
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use std::sync::atomic::Ordering;

fn form() -> OutboundMessage {
    serde_json::from_value(
        json!({"text":"Request","blocks":[{"type":"form","id":"request","fields":[
            {"id":"reason","label":"Reason","required":true,"kind":{"type":"text"}},
            {"id":"amount","label":"Amount","required":false,"kind":{"type":"number"}},
            {"id":"day","label":"Day","required":false,"kind":{"type":"date"}},
            {"id":"scope","label":"Scope","required":true,"kind":{"type":"choice","options":[
                {"id":"private-read","label":"Read"},{"id":"private-write","label":"Write"}]}}
        ]}]}),
    )
    .unwrap()
}
async fn prepare(cards: &Cards, named: bool, bind: bool) -> (PreparedCard, EventRefPayload) {
    let reference = if named {
        &cards.named
    } else {
        &cards.app.event_ref
    };
    let event = reference
        .open(&cards.app.state.keys, now())
        .unwrap()
        .payload;
    let mut card = cards
        .store
        .prepare_lark_form(cards.channel().await.as_ref(), &event, &form())
        .await
        .unwrap();
    if bind {
        cards
            .store
            .bind(
                &mut card,
                Destination {
                    outbound_id: "outbound-1".into(),
                    platform_id: "om_card".into(),
                },
            )
            .await
            .unwrap();
    }
    (card, event)
}
fn callback(cards: &Cards, card: &PreparedCard, values: Value) -> Envelope {
    let container = card.markup["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["tag"] == "form")
        .unwrap();
    let locator =
        &container["elements"].as_array().unwrap().last().unwrap()["behaviors"][0]["value"];
    let mut envelope = Envelope::on(&cards.platform);
    envelope.payload["reply_to_platform_message_id"] = "om_card".into();
    envelope.payload["raw_platform_data"] = json!({"schema":"2.0","header":{"event_id":"form-click","event_type":"card.action.trigger"},"event":{
        "operator":{"open_id":"ou_sender"},"host":"im_message","action":{"tag":"button","value":locator,"form_value":values,"name":"submit"},
        "context":{"open_chat_id":"oc_chat","open_message_id":"om_card"}}});
    sync(&mut envelope);
    envelope
}
fn sync(envelope: &mut Envelope) {
    let event = &envelope.payload["raw_platform_data"]["event"];
    envelope.payload["content"] = json!({"type":"card_action","text":json!({"tag":event["action"]["tag"],
        "value":event["action"]["value"],"form_value":event["action"]["form_value"],"open_message_id":event["context"]["open_message_id"]}).to_string()});
}
fn values() -> Value {
    json!({"f0":"Ready","f1":"12.5","f2":"2024-02-29 +0800","f3":"1"})
}

#[tokio::test]
async fn native_lark_forms_restore_default_and_named_owner_with_exact_results_after_restart() {
    for platform in ["lark", "feishu"] {
        for named in [false, true] {
            let mut cards = Cards::new(platform).await;
            let (card, original) = prepare(&cards, named, true).await;
            cards
                .change(|v| {
                    v["partition"]["key"] = "fixed".into();
                    v["routing"]["default_targets"] = json!([reviewer().as_str()]);
                    v["routing"]["consumers"][0]["name"] = "renamed".into();
                })
                .await;
            cards.restart();
            let mut click = callback(&cards, &card, values());
            if platform == "feishu" {
                click.payload["raw_platform_data"]["event"]["action"]["tag"] = "form_submit".into();
                sync(&mut click);
            }
            let accepted = cards.send(&click).await;
            assert_eq!(accepted.1["status"], "accepted", "{platform}: {accepted:?}");
            let saved = cards.store.load(&card.card.id).await.unwrap();
            let submitted =
                json!({"reason":"Ready","amount":12.5,"day":"2024-02-29","scope":"private-write"});
            assert_eq!(
                serde_json::to_value(&saved.0).unwrap()["consumed"]["submitted"],
                submitted
            );
            click.payload["timestamp"] = "2026-10-01T00:00:00Z".into();
            assert_eq!(cards.send(&click).await, accepted);
            assert_eq!(cards.store.load(&card.card.id).await.unwrap().1, saved.1);
            let requests = cards.app.fixture.recorder.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].activity, requests[1].activity);
            assert_eq!(requests[0].idempotency_key, requests[1].idempotency_key);
            for request in requests.iter() {
                assert_eq!(request.consumer, original.consumer);
                assert_eq!(request.conversation_id, original.conversation_id);
                let opened = request
                    .event_ref
                    .open(&cards.app.state.keys, now())
                    .unwrap()
                    .payload;
                assert_eq!(opened.partition_key, original.partition_key);
                assert!(opened.verifies_activity(&request.activity));
                let ActivityKind::Interaction(result) = &request.activity.kind else {
                    panic!()
                };
                assert_eq!(result.block_id, "request");
                assert_eq!(result.action_id, "request");
                assert_eq!(result.values["fields"], submitted);
                assert!(!result.values.contains_key("selected"));
            }
        }
    }
}

#[tokio::test]
async fn native_lark_forms_refuse_invalid_shapes_without_consuming_then_retry_cas() {
    let mut cards = Cards::new("lark").await;
    let (card, _) = prepare(&cards, true, true).await;
    for invalid in [
        json!({}),
        json!({"f0":"Ready","f3":"private-write"}),
        json!({"f0":"Ready","f3":"01"}),
        json!({"f0":"Ready","f3":["1"]}),
        json!({"f0":"Ready","f1":"NaN","f3":"1"}),
        json!({"f0":"Ready","f2":"2023-02-29","f3":"1"}),
        json!({"f0":"Ready","f3":"1","unknown":"x"}),
        json!({"f0":"x".repeat(1001),"f3":"1"}),
        Value::Null,
    ] {
        assert_ne!(
            cards.send(&callback(&cards, &card, invalid)).await.1["status"],
            "accepted"
        );
        assert!(cards
            .store
            .load(&card.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
    }
    assert!(cards
        .app
        .fixture
        .recorder
        .requests
        .lock()
        .unwrap()
        .is_empty());
    let click = callback(&cards, &card, values());
    cards.records.put_conflicts.store(2, Ordering::SeqCst);
    assert_eq!(cards.send(&click).await.0, 503);
    assert!(cards
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    cards.restart();
    assert_eq!(cards.send(&click).await.1["status"], "accepted");
    let version = cards.store.load(&card.card.id).await.unwrap().1;
    let mut changed = callback(&cards, &card, json!({"f0":"Different","f3":"1"}));
    assert_eq!(
        cards.send(&changed).await.1["reason"],
        "interaction_expired_or_invalid"
    );
    changed = callback(&cards, &card, values());
    changed.payload["raw_platform_data"]["header"]["event_id"] = "another-click".into();
    assert_eq!(
        cards.send(&changed).await.1["reason"],
        "interaction_expired_or_invalid"
    );
    assert_eq!(cards.store.load(&card.card.id).await.unwrap().1, version);
    assert_eq!(cards.app.fixture.recorder.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_lark_forms_retain_actor_message_source_provider_and_revision_fences() {
    for named in [false, true] {
        for case in [
            "actor",
            "message",
            "chat",
            "revision",
            "normalized",
            "source",
            "provider",
        ] {
            let mut cards = Cards::new("feishu").await;
            let (card, _) = prepare(&cards, named, true).await;
            cards.restart();
            let original = cards.channel().await.definition.to_blob_json();
            let mut click = callback(&cards, &card, values());
            match case {
                "actor" => {
                    click.payload["sender"]["platform_id"] = "ou_other".into();
                    click.payload["raw_platform_data"]["event"]["operator"]["open_id"] =
                        "ou_other".into();
                }
                "message" => {
                    click.payload["reply_to_platform_message_id"] = "om_other".into();
                    click.payload["raw_platform_data"]["event"]["context"]["open_message_id"] =
                        "om_other".into();
                }
                "chat" => {
                    click.payload["conversation"]["platform_id"] = "oc_other".into();
                    click.payload["raw_platform_data"]["event"]["context"]["open_chat_id"] =
                        "oc_other".into();
                }
                "revision" => {
                    let locator = &mut click.payload["raw_platform_data"]["event"]["action"]
                        ["value"]["cma_action"];
                    *locator = locator.as_str().unwrap().replace(":1:", ":2:").into();
                }
                "source" => {
                    cards
                        .change(|v| v["sources"][0]["bot_id"] = "ou_other_bot".into())
                        .await
                }
                "provider" => {
                    cards
                        .change(|v| {
                            if named {
                                v["routing"]["consumers"][0]["provider"]["binding_id"] =
                                    "rebound".into();
                            } else {
                                v["provider"]["binding_id"] = "rebound".into();
                            }
                        })
                        .await
                }
                _ => {}
            }
            sync(&mut click);
            if case == "normalized" {
                click.payload["raw_platform_data"]["event"]["action"]["form_value"]["f0"] =
                    "raw-only-change".into();
            }
            let result = cards.send(&click).await;
            if case == "normalized" {
                assert_eq!(result.0, 401, "{case}: {result:?}");
            } else {
                assert_eq!(
                    result.1["reason"], "interaction_expired_or_invalid",
                    "{case}: {result:?}"
                );
            }
            assert!(cards
                .store
                .load(&card.card.id)
                .await
                .unwrap()
                .0
                .consumed
                .is_none());
            cards.change(|v| *v = original).await;
            cards.restart();
            assert!(cards
                .app
                .fixture
                .recorder
                .requests
                .lock()
                .unwrap()
                .is_empty());
            assert_eq!(
                cards.send(&callback(&cards, &card, values())).await.1["status"],
                "accepted"
            );
        }
    }
}

#[tokio::test]
async fn native_lark_forms_require_confirmed_native_authority_and_validate_saved_result() {
    let cards = Cards::new("lark").await;
    let (mut card, _) = prepare(&cards, false, false).await;
    let click = callback(&cards, &card, values());
    assert_eq!(
        cards.send(&click).await.1["reason"],
        "interaction_expired_or_invalid"
    );
    cards
        .store
        .bind(
            &mut card,
            Destination {
                outbound_id: "outbound-1".into(),
                platform_id: "om_card".into(),
            },
        )
        .await
        .unwrap();
    let unconsumed = serde_json::to_value(&card.card).unwrap();
    assert_eq!(cards.send(&click).await.1["status"], "accepted");
    let saved = serde_json::to_value(cards.store.load(&card.card.id).await.unwrap().0).unwrap();
    let owner = cards.channel().await.definition.owner.subject.clone();
    for submitted in [
        Value::Null,
        json!({"reason":"Ready","scope":"missing"}),
        json!({"reason":1,"scope":"private-write"}),
        json!({"reason":"Ready","scope":"private-write","injected":true}),
    ] {
        let mut tampered = saved.clone();
        tampered["consumed"]["submitted"] = submitted;
        let blob = cmaeg_core::seal::seal_interaction(
            &cards.app.state.keys,
            &card.card.id,
            &serde_json::to_vec(&tampered).unwrap(),
        )
        .unwrap();
        cards.records.seed(&card.card.id, &owner, blob);
        assert!(cards.store.load(&card.card.id).await.is_err());
    }
    let mut historical = unconsumed;
    historical["lark"].as_object_mut().unwrap().remove("form");
    let blob = cmaeg_core::seal::seal_interaction(
        &cards.app.state.keys,
        &card.card.id,
        &serde_json::to_vec(&historical).unwrap(),
    )
    .unwrap();
    cards.records.seed(&card.card.id, &owner, blob);
    assert!(cards.store.load(&card.card.id).await.is_ok());
    assert_eq!(
        cards.send(&click).await.1["reason"],
        "interaction_expired_or_invalid"
    );
}
