use super::{
    discord_card_edit_tests::{patches, update},
    discord_card_replacement_fixture::editing,
    discord_media_controls_tests::{media, send},
    discord_media_set_fixture::native,
    intents::DeliveryOutcome,
};
use crate::discord::components_fixture::{message, FixtureState};
use cmaeg_core::message::{NativeMediaKind, NativeMediaSource, OutboundMessage};
use serde_json::json;

fn set(count: usize) -> OutboundMessage {
    let mut request = media(false, "file", b"new media");
    let item = request.native_attachments[0].clone();
    request.native_attachments = (0..count)
        .map(|index| {
            let mut item = item.clone();
            item.filename = Some(format!("item-{index}.bin"));
            item.caption = Some(format!("Item {index}"));
            item.kind = [
                NativeMediaKind::Image,
                NativeMediaKind::File,
                NativeMediaKind::Audio,
                NativeMediaKind::Video,
            ][index % 4];
            item
        })
        .collect();
    request.clear_native_attachments = count == 0;
    request
}

#[tokio::test]
async fn discord_media_sets_change_counts_clear_and_keep_callbacks_after_issuance_stops() {
    for scope in ["default", "named", "thread"] {
        let f = match scope {
            "named" => FixtureState::named().await,
            "thread" => FixtureState::named_thread(11).await,
            _ => FixtureState::new().await,
        };
        let native = native(&f);
        let mut sender = editing(&f);
        sender.relay = sender.relay.with_discord_media_card_sets(true);
        let sent = send(&f, &sender, &set(2), true).await;
        assert!(
            matches!(sent.delivery, DeliveryOutcome::Confirmed { .. }),
            "{scope}: {:?}",
            sent.delivery
        );
        let mut reference = sent.message_ref.unwrap().open(&f.keys()).unwrap().payload;
        let original = f
            .store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        sender.relay = sender
            .relay
            .with_discord_media_controls(false)
            .with_discord_interactions(false);
        for (index, count) in [4, 1, 3, 0].into_iter().enumerate() {
            let request = set(count);
            let previous = reference.clone();
            let result = update(&sender, &f, &reference, &request).await;
            assert!(
                matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
                "{scope}/{count}: {:?}",
                result.delivery
            );
            reference = result
                .message_ref
                .as_ref()
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload;
            let card = f
                .store
                .load(reference.interaction_record_id.as_ref().unwrap())
                .await
                .unwrap()
                .0;
            assert_eq!(
                serde_json::to_value(&card).unwrap()["owner"],
                serde_json::to_value(&original).unwrap()["owner"]
            );
            assert_eq!(card.consumer, original.consumer);
            assert_eq!(card.expires_at, original.expires_at);
            assert_eq!(reference.platform_message_id, previous.platform_message_id);
            assert!(!card.message.clear_native_attachments);
            assert!(card.message.native_attachments.is_empty());
            let identity = card.discord.as_ref().unwrap();
            assert_eq!(identity.media_items().unwrap().len(), count);
            let wire = serde_json::to_value(identity).unwrap();
            assert_eq!(wire.get("media").is_some(), count == 1);
            assert_eq!(wire.get("media_set").is_some(), count > 1);
            assert_eq!(
                native.message.lock().unwrap()["attachments"]
                    .as_array()
                    .unwrap()
                    .len(),
                count
            );
            let replay = update(&sender, &f, &previous, &request).await;
            assert_eq!(
                serde_json::to_value(replay).unwrap(),
                serde_json::to_value(result).unwrap()
            );
            assert_eq!(patches(&f), index + 1);
        }
        // A confirmed clear becomes an ordinary card; it can edit and consume a
        // new callback without any residual authority over the old provider IDs.
        sender.relay = sender
            .relay
            .with_discord_media_card_sets(false)
            .with_discord_media_card_replacements(false)
            .with_discord_media_card_edits(false)
            .with_discord_card_edits(false);
        let result = update(&sender, &f, &reference, &message(false)).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
        let next_ref = result.message_ref.unwrap().open(&f.keys()).unwrap().payload;
        let custom = native.message.lock().unwrap()["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap()
            .to_owned();
        f.receive(f.callback(&custom), 1).await.unwrap();
        let card = f
            .store
            .load(next_ref.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        assert!(card.consumed.is_some());
        assert!(card
            .discord
            .as_ref()
            .unwrap()
            .media_items()
            .unwrap()
            .is_empty());
        assert_eq!(patches(&f), 5);
    }
}

#[tokio::test]
async fn discord_media_sets_refuse_bad_preflight_and_gate_without_consuming_original_callback() {
    for case in [
        "gate",
        "too_many",
        "invalid_last",
        "clear_with_files",
        "multipart",
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let mut sender = editing(&f);
        sender.relay = sender.relay.with_discord_media_card_sets(true);
        let sent = send(&f, &sender, &set(1), true).await;
        let reference = sent.message_ref.unwrap().open(&f.keys()).unwrap().payload;
        let original = f
            .store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        let mut request = set(4);
        match case {
            "gate" => sender.relay = sender.relay.with_discord_media_card_sets(false),
            "too_many" => request = set(5),
            "invalid_last" => {
                request.native_attachments[3].source = NativeMediaSource::Base64 {
                    data: "!!invalid!!".into(),
                }
            }
            "clear_with_files" => request.clear_native_attachments = true,
            "multipart" => request.native_attachments[3].mime_type = Some("invalid\nmime".into()),
            _ => unreachable!(),
        }
        let result = update(&sender, &f, &reference, &request).await;
        assert!(
            !matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
            "{case}"
        );
        assert_eq!(patches(&f), 0);
        assert_eq!(
            serde_json::to_value(f.store.load(&original.id).await.unwrap().0).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        let custom = native.message.lock().unwrap()["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap()
            .to_owned();
        f.receive(f.callback(&custom), 1).await.unwrap();
    }
}

#[tokio::test]
async fn discord_media_sets_unknown_or_wrong_membership_never_replays_or_activates_successor() {
    for (count, fault) in [
        (4, "500"),
        (4, "old_membership"),
        (4, "reverse"),
        (4, "missing"),
        (4, "extra"),
        (4, "duplicate"),
        (4, "last_size"),
        (0, "old_membership"),
        (0, "extra"),
        (0, "controls"),
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let mut sender = editing(&f);
        sender.relay = sender.relay.with_discord_media_card_sets(true);
        let sent = send(&f, &sender, &set(2), true).await;
        let reference = sent.message_ref.unwrap().open(&f.keys()).unwrap().payload;
        *native.fault.lock().unwrap() = Some(fault);
        let request = set(count);
        let result = update(&sender, &f, &reference, &request).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unknown { .. }),
            "{count}/{fault}: {:?}",
            result.delivery
        );
        assert!(result.message_ref.is_none());
        let replay = update(&sender, &f, &reference, &request).await;
        assert_eq!(
            serde_json::to_value(replay).unwrap(),
            serde_json::to_value(result).unwrap()
        );
        assert_eq!(patches(&f), 1);
        let old = f
            .store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        assert!(old.superseded);
        let value = serde_json::to_value(old).unwrap();
        let successor = value["discord_edit"]["successor"].as_str().unwrap();
        let custom = native.message.lock().unwrap()["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let _ = f.receive(f.callback(&custom), 1).await;
        let pending = f.store.load(successor).await.unwrap().0;
        assert!(pending.reference.is_none() && pending.consumed.is_none());
    }
}

#[tokio::test]
async fn discord_media_sets_clear_is_update_only_and_legacy_empty_preserves() {
    let f = FixtureState::new().await;
    let native = native(&f);
    let mut sender = editing(&f);
    sender.relay = sender.relay.with_discord_media_card_sets(true);
    let rejected = send(&f, &sender, &set(0), true).await;
    assert!(!matches!(
        rejected.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(f.rest.memory.calls.lock().unwrap().is_empty());
    let sent = send(&f, &sender, &set(4), true).await;
    let reference = sent.message_ref.unwrap().open(&f.keys()).unwrap().payload;
    let before = native.message.lock().unwrap()["attachments"].clone();
    let mut preserve = set(0);
    preserve.clear_native_attachments = false;
    let result = update(&sender, &f, &reference, &preserve).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    assert_eq!(native.message.lock().unwrap()["attachments"], before);
    assert!(serde_json::from_value::<OutboundMessage>(
        json!({"clear_native_attachments":null,"text":"clear"})
    )
    .is_err());
}

#[tokio::test]
async fn discord_media_sets_recheck_source_consumer_and_bot_after_complete_preparation() {
    for case in ["source", "consumer", "credential", "key", "bot"] {
        let f = FixtureState::named().await;
        let native = native(&f);
        let mut sender = editing(&f);
        sender.relay = sender.relay.with_discord_media_card_sets(true);
        let sent = send(&f, &sender, &set(2), true).await;
        let reference = sent.message_ref.unwrap().open(&f.keys()).unwrap().payload;
        let original = f
            .store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        if case == "bot" {
            let memory = f.rest.memory.clone();
            *native.on_read.lock().unwrap() = Some(Box::new(move || {
                *memory.bot.lock().unwrap() = json!({"id":"78","bot":true});
            }));
        } else {
            let memory = f.setup.fixture.memory.clone();
            let keys = f.keys();
            let mut definition = f.setup.channel.to_blob_json();
            if case == "consumer" {
                definition["routing"]["consumers"][0]["provider"]["binding_id"] = "rebound".into();
            } else if case == "credential" {
                definition["secrets"]["consumer_keys"][0][1] = "changed-provider-credential".into();
                definition["routing"]["consumers"][0]["agent_key_hash"] =
                    crate::channels::definition::hash_secret("changed-provider-credential").into();
            } else if case == "key" {
                definition["routing"]["consumers"][0]["agent_key_hash"] =
                    format!("sha256:{}", "0".repeat(64)).into();
            } else {
                definition["sources"] = json!([]);
            }
            let blob =
                cmaeg_core::seal::seal_blob(&keys, &serde_json::to_vec(&definition).unwrap())
                    .unwrap();
            *native.on_read.lock().unwrap() = Some(Box::new(move || {
                *memory.channel.lock().unwrap() = blob;
            }));
        }
        let result = update(&sender, &f, &reference, &set(4)).await;
        assert!(
            !matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
            "{case}"
        );
        assert_eq!(patches(&f), 0);
        assert_eq!(
            serde_json::to_value(f.store.load(&original.id).await.unwrap().0).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
}

#[tokio::test]
async fn discord_media_sets_release_exact_fence_after_read_only_post_lock_refusal() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let f = FixtureState::new().await;
    let _native = native(&f);
    let mut sender = editing(&f);
    sender.relay = sender.relay.with_discord_media_card_sets(true);
    let sent = send(&f, &sender, &set(2), true).await;
    let reference = sent.message_ref.unwrap().open(&f.keys()).unwrap().payload;
    let card_id = reference.interaction_record_id.as_ref().unwrap().clone();
    let records = &f.setup.fixture.memory;
    let original = records.rows.lock().unwrap()[&card_id].clone();
    let weak = Arc::downgrade(records);
    let keys = f.keys();
    let removed = Arc::new(AtomicBool::new(false));
    let observed = removed.clone();
    let target = card_id.clone();
    // Intercept the successful durable lock PUT, not an ordinal read or timer.
    *records.on_saved.lock().unwrap() = Some(Arc::new(move |id| {
        let memory = weak.upgrade().unwrap();
        let blob = memory.rows.lock().unwrap()[id].blob.clone();
        let opened = cmaeg_core::seal::open_interaction(&keys, id, &blob).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&opened.payload).unwrap();
        if value["kind"] == "discord_message_mutation_v1"
            && value["held"] == true
            && !observed.swap(true, Ordering::SeqCst)
        {
            memory.rows.lock().unwrap().remove(&target);
        }
    }));
    let request = set(4);
    let refused = update(&sender, &f, &reference, &request).await;
    assert!(!matches!(
        refused.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(removed.load(Ordering::SeqCst));
    assert_eq!(patches(&f), 0);
    *records.on_saved.lock().unwrap() = None;
    records.rows.lock().unwrap().insert(card_id, original);
    let retry = update(&sender, &f, &reference, &request).await;
    assert!(
        matches!(retry.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        retry.delivery
    );
    assert_eq!(patches(&f), 1);
}
