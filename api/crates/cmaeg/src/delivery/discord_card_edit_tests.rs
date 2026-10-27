//! Component replacement uses real local REST and encrypted CAS records.
use super::{
    discord_controls_tests::sender,
    gate::recording::RecordingGate,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::{
    discord::components_fixture::{message, multiselect_message, FixtureState},
    interactions::{discord_edit::Admission, PreparedCard},
};
use axum::http::StatusCode;
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};
use serde_json::{json, Value};
use std::sync::Arc;

#[path = "discord_card_edit_failures.rs"]
mod failures;

fn reference(f: &FixtureState, card: &PreparedCard) -> MessageRefPayload {
    card.card
        .reference
        .as_ref()
        .unwrap()
        .open(&f.keys())
        .unwrap()
        .payload
}
fn editing(f: &FixtureState) -> Sender {
    let mut value = sender(f, true);
    value.relay = value
        .relay
        .with_discord_card_edits(true)
        .with_discord_multiselect(true)
        .with_consumer_interactions(true);
    value
}
async fn update(
    sender: &Sender,
    f: &FixtureState,
    reference: &MessageRefPayload,
    message: &OutboundMessage,
) -> IntentReceipt {
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    sender
        .update(
            &SendContext {
                trace: &trace,
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            },
            reference,
            message,
        )
        .await
        .0
}
fn echo(f: &FixtureState, adjust: impl Fn(&mut Value) + Send + Sync + 'static) {
    let memory = Arc::downgrade(&f.rest.memory);
    let metadata = if f.thread_kind.is_some() {
        f.thread_metadata()
    } else {
        match f.context.activity.conversation.kind {
            cmaeg_core::activity::ConversationKind::Private => json!({"id":"99","type":1}),
            _ => json!({"id":"99","type":0,"guild_id":"88"}),
        }
    };
    *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
        let memory = memory.upgrade().unwrap();
        let (status, body) = if call.method == "PATCH" {
            let mut body: Value = serde_json::from_slice(&call.body).unwrap();
            body["id"] = "800".into();
            body["channel_id"] = "99".into();
            body["author"] = json!({"id":"77","bot":true});
            adjust(&mut body);
            (StatusCode::OK, body)
        } else if call.method == "GET" {
            (StatusCode::OK, metadata.clone())
        } else {
            (StatusCode::NO_CONTENT, Value::Null)
        };
        memory.responses.lock().unwrap().push_back((status, body));
    }));
}
fn patches(f: &FixtureState) -> usize {
    f.rest
        .memory
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.method == "PATCH")
        .count()
}

#[tokio::test]
async fn discord_card_edit_gate_preserves_historical_cards_and_requires_no_new_public_authority() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    assert!(serde_json::to_value(&card.card)
        .unwrap()
        .get("discord_edit")
        .is_none());
    let sender = sender(&f, true);
    let before = f.setup.fixture.memory.rows.lock().unwrap().len();
    let result = update(&sender, &f, &reference(&f, &card), &message(true)).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Unsupported));
    assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
}

#[tokio::test]
async fn discord_card_edits_keep_one_destination_owner_thread_expiry_and_exact_receipt() {
    for thread in [false, true] {
        let f = if thread {
            FixtureState::named_thread(11).await
        } else {
            FixtureState::new().await
        };
        let card = f.card(&message(false)).await;
        let original = serde_json::to_value(&card.card).unwrap();
        echo(&f, |_| {});
        let mut sender = editing(&f);
        let first = update(&sender, &f, &reference(&f, &card), &multiselect_message()).await;
        assert!(
            matches!(first.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            first.delivery
        );
        let next_ref = first
            .message_ref
            .as_ref()
            .unwrap()
            .open(&f.keys())
            .unwrap()
            .payload;
        assert!(next_ref.discord_message.is_none());
        let next = f
            .store
            .load(next_ref.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        let saved = serde_json::to_value(&next).unwrap();
        for key in [
            "actor",
            "chat",
            "thread",
            "consumer",
            "discord",
            "expires_at",
            "event_id",
            "destination",
        ] {
            assert_eq!(saved[key], original[key], "{key}");
        }
        assert_eq!(next.revision, 2);
        assert!(f.store.load(&card.card.id).await.unwrap().0.superseded);
        let request = f
            .rest
            .memory
            .calls
            .lock()
            .unwrap()
            .iter()
            .find(|call| call.method == "PATCH")
            .unwrap()
            .clone();
        assert_eq!(request.path, "/api/v10/channels/99/messages/800");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            body["allowed_mentions"],
            json!({"parse":[],"replied_user":false})
        );
        assert!(body.get("message_reference").is_none());
        sender.relay = sender
            .relay
            .with_discord_card_edits(false)
            .with_discord_multiselect(false)
            .with_consumer_interactions(false);
        let replay = update(&sender, &f, &reference(&f, &card), &multiselect_message()).await;
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
        assert_eq!(patches(&f), 1);
        // A different actor cannot consume the edited controls.
        let custom = body["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap();
        let mut wrong = f.callback(custom);
        wrong["data"]["component_type"] = 3.into();
        wrong["data"]["values"] = json!(["1", "0"]);
        if thread {
            wrong["member"]["user"]["id"] = "43".into();
        } else {
            wrong["user"]["id"] = "43".into();
        }
        f.receive(wrong, 2).await.unwrap();
        assert!(f.store.load(&next.id).await.unwrap().0.consumed.is_none());
        let mut click = f.callback(custom);
        click["data"]["component_type"] = 3.into();
        click["data"]["values"] = json!(["1", "0"]);
        f.receive(click.clone(), 3).await.unwrap();
        let consumed = serde_json::to_value(f.store.load(&next.id).await.unwrap().0).unwrap();
        assert_eq!(consumed["consumed"]["selection"], json!([0, 1]));
        f.receive(click, 4).await.unwrap();
        assert_eq!(patches(&f), 1);
        let complete: OutboundMessage = serde_json::from_value(json!({"text":"Done"})).unwrap();
        let cleared = update(&sender, &f, &next_ref, &complete).await;
        assert!(matches!(
            cleared.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let last: Value = {
            let calls = f.rest.memory.calls.lock().unwrap();
            serde_json::from_slice(
                &calls
                    .iter()
                    .rev()
                    .find(|call| call.method == "PATCH")
                    .unwrap()
                    .body,
            )
            .unwrap()
        };
        assert_eq!(last["components"], json!([]));
        let final_ref = cleared
            .message_ref
            .unwrap()
            .open(&f.keys())
            .unwrap()
            .payload;
        let final_card = f
            .store
            .load(final_ref.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        assert_eq!(final_card.revision, 3);
        assert_eq!(final_card.expires_at, card.card.expires_at);
        assert!(final_ref.discord_message.is_none());
        let later: OutboundMessage = serde_json::from_value(json!({"text":"Archived"})).unwrap();
        assert!(matches!(
            update(&sender, &f, &final_ref, &later).await.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
    }
}

#[tokio::test]
async fn discord_card_edits_serialize_different_bodies_and_replay_after_restart() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    echo(&f, |_| {});
    let first = editing(&f);
    let second = editing(&f);
    let reference = reference(&f, &card);
    let a = message(true);
    let mut b = message(false);
    b.text = Some("Different".into());
    let (ra, rb) = tokio::join!(
        update(&first, &f, &reference, &a),
        update(&second, &f, &reference, &b)
    );
    assert_eq!(
        usize::from(matches!(ra.delivery, DeliveryOutcome::Confirmed { .. }))
            + usize::from(matches!(rb.delivery, DeliveryOutcome::Confirmed { .. })),
        1
    );
    assert_eq!(patches(&f), 1);
    let (body, receipt) = if matches!(ra.delivery, DeliveryOutcome::Confirmed { .. }) {
        (&a, ra)
    } else {
        (&b, rb)
    };
    let mut restarted = sender(&f, false);
    restarted.relay =
        restarted
            .relay
            .with_interactions(Arc::new(crate::interactions::InteractionStore::new(
                f.setup.state.resolver.record_client(),
                f.keys(),
                f.provider.client(true, std::time::Duration::from_secs(5)),
            )));
    let replay = update(&restarted, &f, &reference, body).await;
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(receipt).unwrap()
    );
    assert_eq!(patches(&f), 1);
}

#[tokio::test]
async fn discord_card_edit_cas_serializes_with_click_and_preserves_consumed_result() {
    for click_first in [false, true] {
        let f = FixtureState::new().await;
        let card = f.card(&multiselect_message()).await;
        let reference = reference(&f, &card);
        let Admission::Ready(candidate) = f
            .store
            .discord_edit_candidate(&f.channel, &reference, &message(false))
            .await
            .unwrap()
        else {
            panic!()
        };
        let mut click = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        click["data"]["component_type"] = 3.into();
        click["data"]["values"] = json!(["1", "0"]);
        if click_first {
            f.setup.fixture.fail(&f.id(&click), false);
            assert!(f.receive(click.clone(), 2).await.is_err());
        }
        let claim = f
            .store
            .begin_discord_edit(*candidate, &message(false))
            .await;
        if click_first {
            assert!(claim.is_err());
            let saved = f.store.load(&card.card.id).await.unwrap().0;
            assert!(!saved.superseded);
            assert!(saved.consumed.is_some());
            echo(&f, |_| {});
            let result = update(&editing(&f), &f, &reference, &message(false)).await;
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            // Recover the missing inbox after the original acknowledgement deadline.
            f.setup.fixture.advance(3000);
            f.receive(click.clone(), 3).await.unwrap();
            assert!(matches!(
                crate::discord::InboxReader::new(f.setup.state.resolver.record_client(), f.keys())
                    .load(&f.id(&click), f.channel.clone(), f.setup.now())
                    .await
                    .unwrap(),
                crate::discord::InboxLaunch::Launch(_)
            ));
            assert_eq!(
                serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0).unwrap()
                    ["consumed"]["selection"],
                json!([0, 1])
            );
        } else {
            assert!(claim.is_ok());
            f.receive(click, 2).await.unwrap();
            let saved = f.store.load(&card.card.id).await.unwrap().0;
            assert!(saved.superseded);
            assert!(saved.consumed.is_none());
        }
    }
}

#[tokio::test]
async fn discord_modal_retained_forms_edit_with_issuance_off_but_legacy_cards_cannot_adopt() {
    let f = FixtureState::new().await;
    let card = crate::discord::modal_fixture::card(&f).await;
    let legacy = f.card(&message(false)).await;
    let sender = editing(&f);
    assert!(!sender.relay.discord_forms_enabled());
    let mut form = crate::discord::modal_fixture::message();
    form.text = Some("Revised form".into());
    let refused = update(&sender, &f, &reference(&f, &legacy), &form).await;
    assert!(matches!(refused.delivery, DeliveryOutcome::Unsupported));
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    echo(&f, |_| {});
    let result = update(&sender, &f, &reference(&f, &card), &form).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    let next = result
        .message_ref
        .as_ref()
        .unwrap()
        .open(&f.keys())
        .unwrap()
        .payload;
    let retained = f
        .store
        .load(next.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap()
        .0;
    assert!(retained.discord.as_ref().unwrap().form.is_some());
    assert!(retained.discord_form_launch.is_none());
    assert_eq!(retained.revision, 2);
    let call = f
        .rest
        .memory
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|call| call.method == "PATCH")
        .cloned()
        .unwrap();
    let body: Value = serde_json::from_slice(&call.body).unwrap();
    let raw = f.callback(
        body["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    f.receive(raw, 2).await.unwrap();
    let body: Value =
        serde_json::from_slice(&f.rest.memory.calls.lock().unwrap().last().unwrap().body).unwrap();
    assert_eq!(body["type"], 9);
    f.receive(
        crate::discord::modal_fixture::submit(&f, body["data"]["custom_id"].as_str().unwrap()),
        3,
    )
    .await
    .unwrap();
    assert!(f
        .store
        .load(&retained.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
}
