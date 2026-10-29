use super::*;
use crate::{
    delivery::intents::ConfirmedMessagePart,
    ingress::{routing_plan::fixture, routing_store::test_server::Server},
};
use cmaeg_core::{ids::MessageRef, seal::MessageRefPayload};
use serde_json::json;

fn receipt() -> IntentReceipt {
    let mut receipt = uncertain("partial_delivery");
    receipt.message_parts = (0..80)
        .map(|index| {
            let payload: MessageRefPayload = serde_json::from_value(json!({
                "channel_id":"cmaeg1.ch.fixture", "event_id":"telegram:77:700",
                "platform_message_id":(800+index).to_string(), "route_id":"telegram:77:42",
                "consumer":"csm_00000000000a"
            }))
            .unwrap();
            ConfirmedMessagePart {
                index,
                message_ref: MessageRef::seal(&payload, &fixture::keys()).unwrap(),
            }
        })
        .collect();
    receipt
}

async fn claim(store: &Store) -> Admission {
    store
        .claim_scoped(
            "reply_delivery_v1",
            "cmaeg.reply-delivery.v1",
            "exact-consumer-and-event-scope".into(),
            "request-1",
            "content-hash".into(),
            "user-1",
            fixture::NOW + 600,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn split_text_target_receipt_replays_exactly_after_restart_and_fails_on_missing_page() {
    let server = Server::start().await;
    let store = Store::new(server.records.clone(), Arc::new(fixture::keys()));
    let Admission::Claimed(first) = claim(&store).await else {
        panic!("initial claim")
    };
    let id = first.id.clone();
    let expected = receipt();
    store.finish(first, &expected).await.unwrap();
    let restarted = Store::new(server.records.clone(), Arc::new(fixture::keys()));
    let Admission::Replay(replay) = claim(&restarted).await else {
        panic!("retained replay")
    };
    assert_eq!(
        serde_json::to_vec(&replay).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    let page = server
        .memory
        .rows
        .lock()
        .unwrap()
        .keys()
        .find(|key| **key != id)
        .unwrap()
        .clone();
    server.memory.rows.lock().unwrap().remove(&page);
    let root = server.memory.rows.lock().unwrap()[&id].blob.clone();
    let expected = Store::scoped_claim(
        "reply_delivery_v1",
        "cmaeg.reply-delivery.v1",
        "exact-consumer-and-event-scope".into(),
        "request-1",
        "content-hash".into(),
        "user-1",
        fixture::NOW + 600,
    );
    assert!(restarted.replay(&expected, &root).await.is_err());
}

#[tokio::test]
async fn split_text_pages_without_root_pointer_never_allow_resend_or_imply_complete_receipt() {
    let server = Server::start().await;
    let store = Store::new(server.records.clone(), Arc::new(fixture::keys()));
    let Admission::Claimed(first) = claim(&store).await else {
        panic!("initial claim")
    };
    server.fault(first.id.clone(), false, false);
    assert!(store.finish(first, &receipt()).await.is_err());
    assert_eq!(server.memory.rows.lock().unwrap().len(), 11);
    let restarted = Store::new(server.records.clone(), Arc::new(fixture::keys()));
    let Admission::Replay(replay) = claim(&restarted).await else {
        panic!("abandoned send must never be claimed again")
    };
    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    assert!(replay.message_parts.is_empty());
}

#[tokio::test]
async fn native_presentation_receipt_replays_after_issuance_stops_and_kind_changes_conflict() {
    let server = Server::start().await;
    let store = Store::new(server.records.clone(), Arc::new(fixture::keys()));
    let message: OutboundMessage = serde_json::from_value(json!({"native_attachments":[{
        "kind":"voice","source":{"type":"base64","data":"AQIDBA=="}
    }]}))
    .unwrap();
    let request_hash = hash(&message);
    let claim = store
        .claim_scoped(
            "reply_delivery_v1",
            "cmaeg.reply-delivery.v1",
            "same-native-event-consumer".into(),
            "voice-request",
            request_hash.clone(),
            "user-1",
            fixture::NOW + 600,
        )
        .await
        .unwrap();
    let Admission::Claimed(claim) = claim else {
        panic!("first admission");
    };
    let confirmed = IntentReceipt {
        message_parts: vec![],
        message_ref: None,
        delivery: DeliveryOutcome::Confirmed {
            platform_message_id: Some("800".into()),
        },
        rendering: RenderingReport {
            outcome: RenderingOutcome::Rendered,
            blocks: vec![],
        },
    };
    store.finish(claim, &confirmed).await.unwrap();
    let restarted = Store::new(server.records.clone(), Arc::new(fixture::keys()));
    let destination = cmaeg_core::seal::ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "700".into(),
        thread_id: None,
    };
    assert!(!crate::media::presentations::allowed(
        false,
        &destination,
        &message
    ));
    let replay = restarted
        .claim_scoped(
            "reply_delivery_v1",
            "cmaeg.reply-delivery.v1",
            "same-native-event-consumer".into(),
            "voice-request",
            request_hash,
            "user-1",
            fixture::NOW + 600,
        )
        .await
        .unwrap();
    let Admission::Replay(replay) = replay else {
        panic!("must not authorize a resend");
    };
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(confirmed).unwrap()
    );
    let mut changed = message;
    changed.native_attachments[0].kind = cmaeg_core::message::NativeMediaKind::Audio;
    assert!(restarted
        .claim_scoped(
            "reply_delivery_v1",
            "cmaeg.reply-delivery.v1",
            "same-native-event-consumer".into(),
            "voice-request",
            hash(&changed),
            "user-1",
            fixture::NOW + 600
        )
        .await
        .is_err());
}
