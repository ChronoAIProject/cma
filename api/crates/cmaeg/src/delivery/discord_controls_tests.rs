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

pub(super) fn sender(f: &FixtureState, enabled: bool) -> Sender {
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
fn ordinary_channel(f: &FixtureState) {
    f.rest
        .memory
        .responses
        .lock()
        .unwrap()
        .push_back((axum::http::StatusCode::OK, json!({"id":"99","type":1})));
}
#[tokio::test]
async fn native_discord_multiselect_gate_blocks_new_cards_but_preserves_consumption() {
    use crate::discord::components_fixture::multiselect_message;
    for enabled in [false, true] {
        let f = FixtureState::new().await;
        f.rest.respond(
            axum::http::StatusCode::OK,
            json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
        );
        let mut sender = sender(&f, true);
        assert!(!sender.relay.discord_multiselect_enabled());
        if enabled {
            sender.relay = sender.relay.with_discord_multiselect(true);
            ordinary_channel(&f);
        }
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender
            .reply(&ctx, &f.original, &multiselect_message())
            .await;
        if !enabled {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
            assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
            assert!(f.rest.memory.calls.lock().unwrap().is_empty());
            assert!(f.provider.memory.lock().unwrap().seen.is_empty());
            continue;
        }
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let calls = f.rest.memory.calls.lock().unwrap().clone();
        let body: Value = serde_json::from_slice(&calls.last().unwrap().body).unwrap();
        assert_eq!(body["components"][0]["components"][0]["min_values"], 0);
        assert_eq!(body["components"][0]["components"][0]["max_values"], 2);
        let reference = receipt
            .message_ref
            .unwrap()
            .open(&f.keys())
            .unwrap()
            .payload;
        let card_id = reference.interaction_record_id.unwrap();
        sender.relay = sender.relay.with_discord_multiselect(false);
        assert!(!sender.relay.discord_multiselect_enabled());
        let (stopped, _) = sender
            .reply(&ctx, &f.original, &multiselect_message())
            .await;
        assert!(matches!(stopped.delivery, DeliveryOutcome::Unsupported));
        let mut raw = f.callback(
            body["components"][0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        raw["data"]["component_type"] = 3.into();
        raw["data"]["values"] = json!(["1", "0"]);
        f.rest
            .respond(axum::http::StatusCode::NO_CONTENT, Value::Null);
        f.receive(raw, 2).await.unwrap();
        let saved = serde_json::to_value(f.store.load(&card_id).await.unwrap().0).unwrap();
        assert_eq!(saved["consumed"]["selection"], json!([0, 1]));
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), calls.len() + 1);
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
        if enabled {
            ordinary_channel(&f);
        }
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
        assert_eq!(calls.len(), if enabled { 2 } else { 1 });
        let body: Value = serde_json::from_slice(&calls.last().unwrap().body).unwrap();
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert_eq!(
            body["message_reference"]["message_id"],
            f.original.event_id.as_str().rsplit(':').next().unwrap()
        );
        if enabled {
            assert_eq!(calls[0].method, "GET");
            assert_eq!(calls[0].path, "/api/v10/channels/99");
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
            assert!(serde_json::to_value(&card.discord)
                .unwrap()
                .get("thread")
                .is_none());
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
        ordinary_channel(&f);
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
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
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
        ordinary_channel(&f);
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
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
            assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
            assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn native_controls_without_retained_thread_metadata_require_exact_ordinary_channel_proof() {
    use axum::http::StatusCode;
    let cases = [
        (
            StatusCode::OK,
            json!({"id":"99","type":10,"guild_id":"88","parent_id":"100"}),
        ),
        (
            StatusCode::OK,
            json!({"id":"99","type":11,"guild_id":"88","parent_id":"100"}),
        ),
        (
            StatusCode::OK,
            json!({"id":"99","type":12,"guild_id":"88","parent_id":"100"}),
        ),
        (StatusCode::SERVICE_UNAVAILABLE, Value::Null),
        (StatusCode::OK, json!({"id":"100","type":0,"guild_id":"88"})),
        (StatusCode::OK, json!({"id":"99","type":0})),
        (StatusCode::OK, json!({"id":"99","type":1})),
        (StatusCode::OK, json!({"id":"99","type":15,"guild_id":"88"})),
    ];
    for (status, evidence) in cases {
        let f = FixtureState::group().await;
        assert!(f.context.activity.conversation.thread_id.is_none());
        assert!(f.context.activity.thread.is_none());
        let sender = sender(&f, true);
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        f.rest.respond(status, evidence);
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message(false)).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
        assert!(receipt.message_ref.is_none());
        assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
        let calls = f.rest.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "GET");
        assert_eq!(calls[0].path, "/api/v10/channels/99");
        assert!(calls[0]
            .authorization
            .as_deref()
            .unwrap()
            .starts_with("Bot "));
    }
}

#[tokio::test]
async fn native_controls_accept_an_exact_ordinary_server_channel_without_retained_metadata() {
    let f = FixtureState::group().await;
    f.rest.memory.responses.lock().unwrap().push_back((
        axum::http::StatusCode::OK,
        json!({"id":"99","type":0,"guild_id":"88"}),
    ));
    f.rest.respond(
        axum::http::StatusCode::OK,
        json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
    );
    let sender = sender(&f, true);
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender.reply(&ctx, &f.original, &message(true)).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(receipt.message_ref.is_some());
    let calls = f.rest.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].method, "GET");
    assert_eq!(calls[1].method, "POST");
}

#[tokio::test]
async fn native_thread_controls_require_retained_and_authenticated_existing_thread() {
    for kind in [10, 11, 12] {
        for select in [false, true] {
            let f = FixtureState::thread(kind).await;
            let mut outbound = message(select);
            outbound.thread_reply = select;
            assert!(f
                .store
                .prepare_with_context(&f.channel, &f.original, &outbound, f.context.clone())
                .await
                .is_err());
            f.queue_thread_metadata();
            f.rest.respond(
                axum::http::StatusCode::OK,
                json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
            );
            let sender = sender(&f, true);
            let gate = RecordingGate::new(true);
            let trace = tracing::Span::none();
            let ctx = SendContext {
                trace: &trace,
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            };
            let (receipt, _) = sender.reply(&ctx, &f.original, &outbound).await;
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
            let card = f
                .store
                .load(reference.interaction_record_id.as_deref().unwrap())
                .await
                .unwrap()
                .0;
            assert_eq!(serde_json::to_value(&card).unwrap()["thread"], "99");
            let proof = serde_json::to_value(card.discord.unwrap()).unwrap();
            assert_eq!(
                proof["thread"],
                json!({"channel_id":"99","channel_type":kind,"guild_id":"88","parent_id":"100"})
            );
            let calls = f.rest.memory.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].path, "/api/v10/channels/99");
            assert_eq!(calls[1].path, "/api/v10/channels/99/messages");
            let body: Value = serde_json::from_slice(&calls[1].body).unwrap();
            assert_eq!(
                body["components"][0]["components"][0]["type"],
                if select { 3 } else { 2 }
            );
            assert_eq!(body["message_reference"]["channel_id"], "99");
        }
    }
}

#[tokio::test]
async fn native_thread_controls_refuse_missing_or_mismatched_current_metadata_before_card_creation()
{
    for (field, value) in [
        ("id", json!("100")),
        ("type", json!(0)),
        ("guild_id", Value::Null),
        ("parent_id", Value::Null),
        ("parent_id", json!("99")),
        ("parent_id", json!("not-an-id")),
    ] {
        let f = FixtureState::thread(11).await;
        let mut metadata = f.thread_metadata();
        metadata[field] = value;
        f.rest.respond(axum::http::StatusCode::OK, metadata);
        let sender = sender(&f, true);
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message(false)).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
        assert!(receipt.message_ref.is_none());
        assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
    }
}
