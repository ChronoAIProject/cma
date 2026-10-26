use super::{
    gate::recording::RecordingGate,
    intents::DeliveryOutcome,
    pacing::Pacing,
    relay_client::RelayClient,
    send::{SendContext, Sender},
};
use crate::{
    discord::components_fixture::{message, FixtureState},
    turns::observer::NoopObserver,
};
use cmaeg_core::seal::MessageRefPayload;
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

fn sender(f: &FixtureState, enabled: bool) -> Sender {
    let mut relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
        .unwrap()
        .with_discord_interactions(enabled)
        .with_interactions(f.store.clone());
    relay.discord = f.rest.api.clone();
    Sender {
        custom: super::custom::CustomDelivery::new(
            "https://gateway.example".into(),
            Arc::default(),
        ),
        keys: f.keys(),
        relay,
        pacing: Pacing::default(),
        consumed: Mutex::default(),
        observer: Arc::new(NoopObserver),
    }
}
#[tokio::test]
async fn native_discord_controls_are_explicit_and_confirmed_messages_bind_one_card() {
    for enabled in [false, true] {
        let f = FixtureState::new().await;
        f.rest.respond(
            axum::http::StatusCode::OK,
            json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
        );
        let sender = sender(&f, enabled);
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message(false)).await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let calls = f.rest.memory.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let body: Value = serde_json::from_slice(&calls[0].body).unwrap();
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert_eq!(
            body["message_reference"]["message_id"],
            f.original.event_id.as_str().rsplit(':').next().unwrap()
        );
        if enabled {
            assert_eq!(body["components"][0]["components"][0]["type"], 2);
            assert_eq!(body["components"][0]["components"][1]["style"], 4);
            assert!(!body.to_string().contains("private-yes"));
            let reference: MessageRefPayload = receipt
                .message_ref
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload;
            let card = f
                .store
                .load(reference.interaction_record_id.as_deref().unwrap())
                .await
                .unwrap()
                .0;
            assert!(card.discord.is_some());
            assert!(card.reference.is_some());
            assert_eq!(card.destination.unwrap().platform_id, "800");
        } else {
            assert!(body.get("components").is_none());
            assert!(body["content"].as_str().unwrap().contains("Yes"));
            assert!(receipt.message_ref.is_none());
        }
    }
}
#[tokio::test]
async fn native_discord_controls_require_send_commit_and_do_not_bind_uncertain_delivery() {
    for allowed in [false, true] {
        let f = FixtureState::new().await;
        f.rest
            .respond(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({}));
        let sender = sender(&f, true);
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let gate = RecordingGate::new(allowed);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message(true)).await;
        assert!(receipt.message_ref.is_none());
        if allowed {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
            assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
            assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn native_discord_named_controls_also_require_the_consumer_issuance_gate() {
    for consumer_enabled in [false, true] {
        let f = FixtureState::named().await;
        f.rest.respond(
            axum::http::StatusCode::OK,
            json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
        );
        let mut sender = sender(&f, true);
        sender.relay = sender.relay.with_consumer_interactions(consumer_enabled);
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message(true)).await;
        if consumer_enabled {
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            let reference = receipt
                .message_ref
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload;
            assert_eq!(reference.consumer, f.original.consumer);
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
            assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
            assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        }
    }
}
