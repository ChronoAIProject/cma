use super::lark_fixture::*;
use crate::{interactions::InteractionStore, records::RecordClient};
use cmaeg_core::channel::Platform;
use serde_json::{json, Value};

fn card() -> Value {
    json!({"text":"Choose","blocks":[{"type":"buttons","id":"decision","options":[{"id":"yes","label":"Yes"}]}]})
}
fn store(f: &App) -> InteractionStore {
    InteractionStore::new(
        RecordClient::new(&f.backend.base_url, "cma-service").unwrap(),
        f.state.keys.clone(),
        f.state.providers.clone(),
    )
}
#[tokio::test]
async fn lark_card_delete_fences_fresh_callbacks_and_retains_exact_consumed_recovery() {
    for consumed in [false, true] {
        let f = App::new(Platform::Lark).await;
        let reference = f.send(card()).await.message_ref.unwrap();
        let payload = reference.open(&f.state.keys).unwrap().payload;
        let id = payload.interaction_record_id.as_ref().unwrap();
        let cards = store(&f);
        let data = format!("cl1:{}:1:0", id.replace('-', ""));
        let channel = f
            .state
            .resolver
            .resolve_current(f.event.channel_id.as_str())
            .await
            .unwrap();
        let mut activity = f.activity.clone();
        activity.event_id = "click-one".parse().unwrap();
        if consumed {
            cards
                .consume(
                    &channel,
                    &activity,
                    "route-one",
                    "callback-one",
                    &data,
                    "om_sent",
                    &|_| true,
                )
                .await
                .unwrap();
        }
        let result = f
            .state
            .delete_message(f.caller(), &reference, "delete")
            .await
            .unwrap();
        assert_eq!(result.delivery.kind(), "confirmed");
        assert!(cards.load(id).await.unwrap().0.superseded);
        assert!(
            !f.state
                .message_effect_capabilities(f.caller(), &reference)
                .await
                .unwrap()
                .delete
        );
        let retry = cards
            .consume(
                &channel,
                &activity,
                "route-one",
                "callback-one",
                &data,
                "om_sent",
                &|_| true,
            )
            .await;
        assert_eq!(retry.is_ok(), consumed);
        activity.event_id = "different-event".parse().unwrap();
        assert!(cards
            .consume(
                &channel,
                &activity,
                "route-one",
                "callback-one",
                &data,
                "om_sent",
                &|_| true
            )
            .await
            .is_err());
    }
}
#[tokio::test]
async fn lark_rejected_delete_restores_callbacks_but_uncertain_delete_keeps_them_fenced() {
    for (status, body, expected) in [
        (200, json!({"code":12345}), "rejected"),
        (502, json!({}), "unknown"),
    ] {
        let f = App::new(Platform::Lark).await;
        let reference = f.send(card()).await.message_ref.unwrap();
        let payload = reference.open(&f.state.keys).unwrap().payload;
        let id = payload.interaction_record_id.as_ref().unwrap();
        f.override_response("DELETE", MESSAGE, status, body);
        assert_eq!(
            f.state
                .delete_message(f.caller(), &reference, "delete")
                .await
                .unwrap()
                .delivery
                .kind(),
            expected
        );
        let cards = store(&f);
        assert_eq!(
            cards.load(id).await.unwrap().0.superseded,
            expected == "unknown"
        );
        let channel = f
            .state
            .resolver
            .resolve_current(f.event.channel_id.as_str())
            .await
            .unwrap();
        let data = format!("cl1:{}:1:0", id.replace('-', ""));
        assert_eq!(
            cards
                .consume(
                    &channel,
                    &f.activity,
                    "route-one",
                    "callback",
                    &data,
                    "om_sent",
                    &|_| true
                )
                .await
                .is_ok(),
            expected == "rejected"
        );
    }
}
#[tokio::test]
async fn lark_unknown_effect_blocks_text_and_card_edit_dispatch() {
    for message in [json!({"text":"hello"}), card()] {
        let f = App::new(Platform::Lark).await;
        let reference = f.send(message.clone()).await.message_ref.unwrap();
        let payload = reference.open(&f.state.keys).unwrap().payload;
        if let Some(id) = payload.interaction_record_id.as_ref() {
            let channel = f
                .state
                .resolver
                .resolve_current(f.event.channel_id.as_str())
                .await
                .unwrap();
            let data = format!("cl1:{}:1:0", id.replace('-', ""));
            store(&f)
                .consume(
                    &channel,
                    &f.activity,
                    "route-one",
                    "callback",
                    &data,
                    "om_sent",
                    &|_| true,
                )
                .await
                .unwrap();
        }
        f.override_response("DELETE", MESSAGE, 502, json!({}));
        assert_eq!(
            f.state
                .delete_message(f.caller(), &reference, "delete")
                .await
                .unwrap()
                .delivery
                .kind(),
            "unknown"
        );
        let before = f.memory.requests.lock().unwrap().len();
        let result = f
            .state
            .update(
                f.caller(),
                &reference,
                serde_json::from_value(message).unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(result.delivery.kind(), "confirmed");
        assert_eq!(f.memory.requests.lock().unwrap().len(), before);
    }
}

#[tokio::test]
async fn lark_card_delete_and_callback_cas_race_preserves_only_the_accepted_event() {
    let f = App::new(Platform::Lark).await;
    let reference = f.send(card()).await.message_ref.unwrap();
    let payload = reference.open(&f.state.keys).unwrap().payload;
    let id = payload.interaction_record_id.as_ref().unwrap();
    let data = format!("cl1:{}:1:0", id.replace('-', ""));
    let cards = store(&f);
    let channel = f
        .state
        .resolver
        .resolve_current(f.event.channel_id.as_str())
        .await
        .unwrap();
    let mut activity = f.activity.clone();
    activity.event_id = "racing-click".parse().unwrap();
    let admit: &crate::interactions::Admit = &|_| true;
    let (deleted, consumed) = tokio::join!(
        f.state.delete_message(f.caller(), &reference, "delete"),
        cards.consume(
            &channel,
            &activity,
            "route-one",
            "callback-one",
            &data,
            "om_sent",
            admit
        ),
    );
    assert_eq!(deleted.unwrap().delivery.kind(), "confirmed");
    let (saved, _) = cards.load(id).await.unwrap();
    assert!(saved.superseded);
    assert_eq!(saved.consumed.is_some(), consumed.is_ok());
    assert_eq!(
        cards
            .consume(
                &channel,
                &activity,
                "route-one",
                "callback-one",
                &data,
                "om_sent",
                admit
            )
            .await
            .is_ok(),
        consumed.is_ok()
    );
    activity.event_id = "fresh-click".parse().unwrap();
    assert!(cards
        .consume(
            &channel,
            &activity,
            "route-one",
            "callback-one",
            &data,
            "om_sent",
            admit
        )
        .await
        .is_err());
    assert_eq!(f.writes().len(), 1);
}

#[tokio::test]
async fn lark_unconsumed_card_edit_refusal_does_not_fence_later_deletion() {
    let f = App::new(Platform::Lark).await;
    let reference = f.send(card()).await.message_ref.unwrap();
    let result = f
        .state
        .update(
            f.caller(),
            &reference,
            serde_json::from_value(card()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result.delivery.kind(), "rejected");
    assert!(f.writes().is_empty());
    assert_eq!(
        f.state
            .delete_message(f.caller(), &reference, "delete")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    assert_eq!(f.writes().len(), 1);
}
