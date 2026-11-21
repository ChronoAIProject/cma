use super::{discord_card_fixture::*, *};
use crate::{
    discord::{
        components_fixture::{message, FixtureState},
        InboxLaunch, InboxReader,
    },
    interactions::{discord_consume::DiscordClick, PreparedCard},
};
use axum::http::StatusCode;
use serde_json::json;
use std::time::Duration;

async fn consume(f: &FixtureState, card: &PreparedCard, callback: &str, expired: bool) -> bool {
    let mut activity = f.context.activity.clone();
    activity.event_id = format!("discord:77:interaction:{callback}")
        .parse()
        .unwrap();
    f.store
        .consume_discord(
            &f.channel,
            &activity,
            DiscordClick {
                id: callback,
                custom_id: card.markup[0]["components"][0]["custom_id"]
                    .as_str()
                    .unwrap(),
                component_type: 2,
                values: &[],
                message: "800",
                thread: card.card.discord.as_ref().unwrap().thread.as_ref(),
            },
            if expired {
                tokio::time::Instant::now() - Duration::from_secs(1)
            } else {
                tokio::time::Instant::now() + Duration::from_secs(2)
            },
        )
        .await
        .is_ok()
}
#[tokio::test]
async fn card_effects_delete_and_click_cas_order_preserves_the_single_consumed_winner() {
    for click_first in [false, true] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        let reference = reference(&card).open(&f.keys()).unwrap().payload;
        let candidate = f
            .store
            .discord_card_candidate(&f.channel, &reference)
            .await
            .unwrap();
        let lock = f.store.mutation_store();
        let held = lock
            .acquire(
                candidate.mutation_destination(),
                &f.channel.definition.owner.subject,
                card.card.expires_at,
            )
            .await
            .unwrap()
            .unwrap();
        let claim = uuid::Uuid::new_v4().to_string();
        if click_first {
            assert!(consume(&f, &card, "900", false).await);
            // The stale delete CAS loses to the consumed click, then reloads it.
            assert!(f
                .store
                .begin_discord_effect(candidate, Some(&claim), true)
                .await
                .is_err());
            let candidate = f
                .store
                .discord_card_candidate(&f.channel, &reference)
                .await
                .unwrap();
            f.store
                .begin_discord_effect(candidate, Some(&claim), true)
                .await
                .unwrap();
            assert!(consume(&f, &card, "900", true).await);
            assert!(!consume(&f, &card, "901", false).await);
        } else {
            f.store
                .begin_discord_effect(candidate, Some(&claim), true)
                .await
                .unwrap();
            assert!(!consume(&f, &card, "900", false).await);
        }
        let saved = f.store.load(&card.card.id).await.unwrap().0;
        assert_eq!(saved.consumed.is_some(), click_first);
        assert!(saved.superseded);
        f.store
            .finish_discord_delete(
                &card.card.id,
                &claim,
                &crate::delivery::intents::DeliveryOutcome::Confirmed {
                    platform_message_id: Some("800".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(consume(&f, &card, "900", true).await, click_first);
        lock.release(held).await.unwrap();
    }
}

#[tokio::test]
async fn card_effects_original_callback_inbox_recovers_after_confirmed_or_uncertain_delete_and_ack_deadline(
) {
    for status in [StatusCode::NO_CONTENT, StatusCode::INTERNAL_SERVER_ERROR] {
        let f = FixtureState::named_thread(11).await;
        let card = f.card(&message(false)).await;
        let raw = f.callback(
            card.markup[0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        let inbox = f.id(&raw);
        f.setup.fixture.fail(&inbox, false);
        f.queue_thread_metadata();
        assert!(f.receive(raw.clone(), 2).await.is_err());
        let consumed =
            serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0.consumed).unwrap();
        assert!(!consumed.is_null());
        echo(&f, status, json!([]));
        let tools = state(&f, true);
        let result = tools
            .delete_message(caller(&f), &reference(&card), "delete")
            .await
            .unwrap();
        assert_eq!(
            result.delivery.kind(),
            if status == StatusCode::NO_CONTENT {
                "confirmed"
            } else {
                "unknown"
            }
        );
        let before = writes(&f);
        f.setup.fixture.advance(3000);
        f.receive(raw.clone(), 2).await.unwrap();
        let reader = InboxReader::new(tools.resolver.record_client(), f.keys());
        let InboxLaunch::Launch(launch) = reader
            .load(&inbox, f.channel.clone(), f.setup.now())
            .await
            .unwrap()
        else {
            panic!("original accepted callback must recover")
        };
        assert_eq!(launch.request.consumer, f.original.consumer);
        assert_eq!(launch.request.conversation_id, f.original.conversation_id);
        assert_eq!(
            serde_json::to_value(f.store.load(&card.card.id).await.unwrap().0.consumed).unwrap(),
            consumed
        );
        assert_eq!(
            writes(&f),
            before,
            "late recovery does not ACK or mutate Discord again"
        );
        let mut different = raw;
        different["id"] = f.setup.fixture.message_id().into();
        f.receive(different.clone(), 3).await.unwrap();
        assert!(!f
            .setup
            .fixture
            .memory
            .rows
            .lock()
            .unwrap()
            .contains_key(&f.id(&different)));
    }
}

#[tokio::test]
async fn card_effects_reaction_fence_blocks_edit_delete_but_allows_callback_consumption() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let payload = reference.open(&f.keys()).unwrap().payload;
    let candidate = f
        .store
        .discord_card_candidate(&f.channel, &payload)
        .await
        .unwrap();
    let serial = f.store.mutation_store();
    let held = serial
        .acquire(
            candidate.mutation_destination(),
            &f.channel.definition.owner.subject,
            card.card.expires_at,
        )
        .await
        .unwrap()
        .unwrap();
    f.store
        .begin_discord_effect(candidate, None, true)
        .await
        .unwrap();
    let mut tools = state(&f, false);
    let _engine = engine(&f, &mut tools);
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    assert_eq!(
        tools
            .update(caller(&f), &reference, message(true))
            .await
            .unwrap()
            .delivery
            .kind(),
        "rejected"
    );
    assert_eq!(
        tools
            .delete_message(caller(&f), &reference, "busy")
            .await
            .unwrap()
            .delivery
            .kind(),
        "rejected"
    );
    assert!(consume(&f, &card, "900", false).await);
    assert!(writes(&f).is_empty());
    serial.release(held).await.unwrap();
    assert_eq!(
        tools
            .delete_message(caller(&f), &reference, "later")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    assert!(consume(&f, &card, "900", true).await);
}

#[tokio::test]
async fn card_effects_edit_unknown_holds_same_lock_across_successor_revision() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let mut tools = state(&f, true);
    let _engine = engine(&f, &mut tools);
    assert!(
        tools
            .message_effect_capabilities(caller(&f), &reference)
            .await
            .unwrap()
            .delete
    );
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    let memory = f.setup.fixture.memory.clone();
    let prior = f.rest.memory.on_call.lock().unwrap().clone().unwrap();
    // Lose the old card's final edit receipt after the successor was bound.
    let old = card.card.id.clone();
    *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
        prior(call);
        if call.method == "PATCH" {
            *memory.fault.lock().unwrap() = Some((old.clone(), false));
        }
    }));
    assert_eq!(
        tools
            .update(caller(&f), &reference, message(true))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    let rows = f
        .setup
        .fixture
        .memory
        .rows
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut successor = None;
    for id in rows {
        if let Ok((value, _)) = f.store.load(&id).await {
            if value.revision == 2 && value.discord.is_some() {
                successor = value.reference;
            }
        }
    }
    let successor = successor.expect("bound successor retained after lost old receipt");
    assert_eq!(
        tools
            .delete_message(caller(&f), &successor, "new")
            .await
            .unwrap()
            .delivery
            .kind(),
        "rejected"
    );
    assert_eq!(writes(&f).len(), 1);
}

#[tokio::test]
async fn card_effects_definite_rejection_restores_controls_only_after_successful_final_cas() {
    for status in [StatusCode::FORBIDDEN, StatusCode::TOO_MANY_REQUESTS] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        let reference = reference(&card);
        let tools = state(&f, true);
        echo(&f, status, json!([]));
        let result = tools
            .delete_message(caller(&f), &reference, "no-write")
            .await
            .unwrap();
        assert_eq!(
            result.delivery.kind(),
            if status == StatusCode::FORBIDDEN {
                "rejected"
            } else {
                "rate_limited"
            }
        );
        assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
        assert!(consume(&f, &card, "900", false).await);
    }
}

#[tokio::test]
async fn card_effects_concurrent_duplicate_delete_has_one_native_write() {
    let f = FixtureState::new().await;
    let card = f.card(&message(false)).await;
    let reference = reference(&card);
    let tools = state(&f, true);
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    let (a, b) = tokio::join!(
        tools.delete_message(caller(&f), &reference, "duplicate"),
        tools.delete_message(caller(&f), &reference, "duplicate")
    );
    let outcomes = [a.unwrap().delivery.kind(), b.unwrap().delivery.kind()];
    assert!(outcomes.contains(&"confirmed"));
    assert!(outcomes
        .iter()
        .all(|value| matches!(*value, "confirmed" | "unknown")));
    assert_eq!(writes(&f).len(), 1);
}

#[tokio::test]
async fn card_effects_deleted_multiselect_recovers_only_exact_canonical_consumed_selection() {
    let f = FixtureState::named().await;
    let card = f
        .card(&crate::discord::components_fixture::multiselect_message())
        .await;
    let mut activity = f.context.activity.clone();
    activity.event_id = "discord:77:interaction:900".parse().unwrap();
    let custom = card.markup[0]["components"][0]["custom_id"]
        .as_str()
        .unwrap();
    let values = vec!["1".into(), "0".into()];
    let click = |values| DiscordClick {
        id: "900",
        custom_id: custom,
        component_type: 3,
        values,
        message: "800",
        thread: None,
    };
    f.store
        .consume_discord(
            &f.channel,
            &activity,
            click(&values),
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
    echo(&f, StatusCode::NO_CONTENT, json!([]));
    state(&f, true)
        .delete_message(caller(&f), &reference(&card), "delete")
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() - Duration::from_secs(1);
    let reordered = vec!["0".into(), "1".into()];
    assert!(f
        .store
        .consume_discord(&f.channel, &activity, click(&reordered), deadline)
        .await
        .is_ok());
    let different = vec!["0".into()];
    assert!(f
        .store
        .consume_discord(&f.channel, &activity, click(&different), deadline)
        .await
        .is_err());
}
