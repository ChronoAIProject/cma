//! One multipart write, exact successor proof, replay and pending-state fences.
use super::{
    discord_card_edit_tests::{patches, update},
    discord_card_replacement_fixture::{corrupt, editing, issued, native, replacement},
    intents::DeliveryOutcome,
};
use crate::{
    discord::components_fixture::{message, FixtureState},
    interactions::discord_edit::{Admission, Replacement},
};
use cmaeg_core::message::OutboundMessage;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[tokio::test]
async fn discord_media_card_replacement_keeps_scope_and_confirms_new_file_before_renewal() {
    for scope in ["default", "named", "thread"] {
        let f = match scope {
            "named" => FixtureState::named().await,
            "thread" => FixtureState::named_thread(11).await,
            _ => FixtureState::new().await,
        };
        let native = native(&f);
        let mut sender = editing(&f);
        let mut reference = issued(&f, &sender).await;
        let original = f
            .store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        for (index, kind) in ["file", "image", "audio", "video"].into_iter().enumerate() {
            let previous = reference.clone();
            let request = replacement(kind);
            let result = update(&sender, &f, &previous, &request).await;
            assert!(
                matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
                "{scope}/{kind}: {:?}",
                result.delivery
            );
            reference = result
                .message_ref
                .as_ref()
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload;
            assert_eq!(reference.platform_message_id, "800");
            let next = f
                .store
                .load(reference.interaction_record_id.as_ref().unwrap())
                .await
                .unwrap()
                .0;
            assert_eq!(next.revision, (index + 2) as u64);
            let current_value = serde_json::to_value(&next).unwrap();
            let original_value = serde_json::to_value(&original).unwrap();
            for field in [
                "owner",
                "actor",
                "channel_id",
                "event_id",
                "route_id",
                "chat",
                "thread",
            ] {
                assert_eq!(current_value[field], original_value[field]);
            }
            assert_eq!(next.expires_at, original.expires_at);
            assert_eq!(next.consumer, original.consumer);
            assert!(next.message.native_attachments.is_empty());
            let identity = serde_json::to_value(&next.discord).unwrap();
            assert_eq!(identity["media"]["kind"], kind);
            assert_eq!(
                identity["media"]["confirmed"]["attachment_id"],
                (901 + index).to_string()
            );
            assert_eq!(
                identity["media"]["sha256"],
                format!("{:x}", Sha256::digest(b"replacement bytes"))
            );
            assert!(identity["media"].get("pending_replacement").is_none());
            assert_eq!(
                native.message.lock().unwrap()["attachments"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            sender.relay = sender.relay.with_discord_media_card_replacements(false);
            let replay = update(&sender, &f, &previous, &request).await;
            assert_eq!(
                serde_json::to_value(&replay).unwrap(),
                serde_json::to_value(&result).unwrap()
            );
            assert_eq!(patches(&f), index + 1);
            sender.relay = sender.relay.with_discord_media_card_replacements(true);
        }
        sender.relay = sender
            .relay
            .with_discord_media_card_replacements(false)
            .with_discord_media_card_edits(false)
            .with_discord_card_edits(false);
        let preserve = update(&sender, &f, &reference, &message(true)).await;
        assert!(matches!(
            preserve.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert_eq!(patches(&f), 5);
        let custom = native.message.lock().unwrap()["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut callback = f.callback(&custom);
        callback["data"]["component_type"] = 3.into();
        callback["data"]["values"] = json!(["1"]);
        f.receive(callback, 2).await.unwrap();
        let reference = preserve
            .message_ref
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
        assert!(card.consumed.is_some());
        assert_eq!(
            card.discord.unwrap().conversation_id,
            f.original.conversation_id
        );
    }
}

#[tokio::test]
async fn discord_media_card_replacement_bad_native_echo_stays_pending_without_callback_or_replay_write(
) {
    for field in [
        "old_id", "size", "filename", "mime", "caption", "extra", "missing", "controls", "content",
        "reply", "type", "flags", "spoiler", "channel", "author", "500",
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let sender = editing(&f);
        let original = issued(&f, &sender).await;
        *native.fault.lock().unwrap() = Some(field);
        let request = replacement("file");
        let result = update(&sender, &f, &original, &request).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unknown { .. }),
            "{field}: {:?}",
            result.delivery
        );
        assert!(result.message_ref.is_none());
        let old = f
            .store
            .load(original.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        assert!(old.superseded);
        let value = serde_json::to_value(old).unwrap();
        let successor = value["discord_edit"]["successor"].as_str().unwrap();
        let pending = f.store.load(successor).await.unwrap().0;
        assert!(pending.reference.is_none());
        assert!(pending.consumed.is_none());
        let value = serde_json::to_value(&pending).unwrap();
        assert_eq!(value["discord"]["media"]["pending_replacement"], true);
        assert!(value["discord"]["media"].get("confirmed").is_none());
        let custom = native.message.lock().unwrap()["components"][0]["components"][0]["custom_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let _ = f.receive(f.callback(&custom), 2).await;
        assert!(f.store.load(successor).await.unwrap().0.consumed.is_none());
        let replay = update(&sender, &f, &original, &request).await;
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&result).unwrap()
        );
        assert_eq!(patches(&f), 1, "{field}");
    }
}

#[tokio::test]
async fn discord_media_card_replacement_refuses_changed_native_source_before_superseding() {
    for field in [
        "size", "filename", "mime", "caption", "missing", "controls", "content", "reply",
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let sender = editing(&f);
        let reference = issued(&f, &sender).await;
        corrupt(&mut native.message.lock().unwrap(), field);
        let result = update(&sender, &f, &reference, &replacement("image")).await;
        assert!(
            !matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
            "{field}"
        );
        assert_eq!(patches(&f), 0);
        assert!(
            !f.store
                .load(reference.interaction_record_id.as_ref().unwrap())
                .await
                .unwrap()
                .0
                .superseded
        );
    }
}

#[tokio::test]
async fn discord_media_card_replacement_requires_gate_existing_proof_single_file_and_current_owner()
{
    for case in [
        "gate", "source", "owner", "legacy", "plain", "multiple", "forms", "reply",
    ] {
        let mut f = FixtureState::named().await;
        let _native = native(&f);
        let mut sender = editing(&f);
        if case == "legacy" {
            sender.relay = sender.relay.with_discord_media_card_edits(false);
        }
        let reference = if case == "plain" {
            f.card(&message(false))
                .await
                .card
                .reference
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload
        } else {
            issued(&f, &sender).await
        };
        let mut request = replacement("file");
        sender.relay = sender.relay.with_discord_media_card_edits(true);
        match case {
            "gate" => sender.relay = sender.relay.with_discord_media_card_replacements(false),
            "source" => {
                Arc::make_mut(&mut f.channel)
                    .definition
                    .secrets
                    .discord_bot_token = None
            }
            "owner" => Arc::make_mut(&mut f.channel).definition.owner.subject = "other".into(),
            "multiple" => request
                .native_attachments
                .push(request.native_attachments[0].clone()),
            "forms" => request
                .blocks
                .extend(crate::discord::modal_fixture::message().blocks),
            "reply" => {
                request.reply_to =
                    Some(cmaeg_core::ids::MessageRef::seal(&reference, &f.keys()).unwrap())
            }
            _ => {}
        }
        let calls = f.rest.memory.calls.lock().unwrap().len();
        let result = update(&sender, &f, &reference, &request).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unsupported),
            "{case}: {:?}",
            result.delivery
        );
        assert_eq!(patches(&f), 0);
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), calls);
        assert!(
            !f.store
                .load(reference.interaction_record_id.as_ref().unwrap())
                .await
                .unwrap()
                .0
                .superseded
        );
    }
}

#[tokio::test]
async fn discord_media_card_replacement_pending_cas_cannot_issue_authority_and_preserves_complete_hash(
) {
    let f = FixtureState::new().await;
    let _native = native(&f);
    let sender = editing(&f);
    let reference = issued(&f, &sender).await;
    let bytes = vec![b'x'; 64 * 1024];
    let request = super::discord_media_controls_tests::media(false, "file", &bytes);
    let files = crate::discord::outbound_media::prepare(&request.native_attachments)
        .await
        .unwrap();
    let Admission::Ready(first) = f
        .store
        .discord_edit_candidate(&f.channel, &reference, &request)
        .await
        .unwrap()
    else {
        panic!("ready candidate expected")
    };
    let Admission::Ready(racing) = f
        .store
        .discord_edit_candidate(&f.channel, &reference, &request)
        .await
        .unwrap()
    else {
        panic!("ready candidate expected")
    };
    let Admission::Ready(deleting) = f
        .store
        .discord_edit_candidate(&f.channel, &reference, &request)
        .await
        .unwrap()
    else {
        panic!("ready candidate expected")
    };
    let mut claim = f
        .store
        .begin_discord_media_replacement(
            *first,
            &request,
            Replacement::prepare(&request, &files[0]).unwrap(),
        )
        .await
        .unwrap();
    assert!(f
        .store
        .begin_discord_media_replacement(
            *racing,
            &request,
            Replacement::prepare(&request, &files[0]).unwrap()
        )
        .await
        .is_err());
    assert!(f
        .store
        .begin_discord_effect(*deleting, Some(&uuid::Uuid::new_v4().to_string()), true)
        .await
        .is_err());
    let record =
        serde_json::to_vec(&f.store.load(&claim.prepared.card.id).await.unwrap().0).unwrap();
    assert!(record.len() < 16 * 1024);
    let value: serde_json::Value = serde_json::from_slice(&record).unwrap();
    assert!(value["message"].get("native_attachments").is_none());
    assert_eq!(value["discord"]["media"]["bytes"], 64 * 1024);
    let destination = claim.prepared.card.destination.clone().unwrap();
    assert!(f
        .store
        .bind(&mut claim.prepared, destination)
        .await
        .is_err());
    let replay = update(&sender, &f, &reference, &request).await;
    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    let different = replacement("file");
    assert!(matches!(
        update(&sender, &f, &reference, &different).await.delivery,
        DeliveryOutcome::Unsupported
    ));
    assert_eq!(patches(&f), 0);
}

#[tokio::test]
async fn discord_media_card_replacement_binding_storage_failure_never_resends_native_write() {
    for fault in ["bind_before", "bind_after"] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let sender = editing(&f);
        let original = issued(&f, &sender).await;
        *native.fault.lock().unwrap() = Some(fault);
        let request = replacement("file");
        let result = update(&sender, &f, &original, &request).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unknown { .. }),
            "{fault}: {:?}",
            result.delivery
        );
        assert!(result.message_ref.is_none());
        let replay = update(&sender, &f, &original, &request).await;
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&result).unwrap()
        );
        assert_eq!(patches(&f), 1);
    }
}

#[tokio::test]
async fn discord_media_card_replacement_can_remove_controls_and_resume_edits() {
    let f = FixtureState::new().await;
    let native = native(&f);
    let sender = editing(&f);
    let original = issued(&f, &sender).await;
    let mut request: OutboundMessage =
        serde_json::from_value(json!({"text":"Updated file"})).unwrap();
    request.native_attachments = replacement("file").native_attachments;
    let result = update(&sender, &f, &original, &request).await;
    assert!(
        matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        result.delivery
    );
    assert_eq!(native.message.lock().unwrap()["components"], json!([]));
    let reference = result.message_ref.unwrap().open(&f.keys()).unwrap().payload;
    let restored = update(&sender, &f, &reference, &message(false)).await;
    assert!(matches!(
        restored.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(patches(&f), 2);
}

#[tokio::test]
async fn discord_media_card_replacement_replay_never_refetches_expired_source_url() {
    let f = FixtureState::new().await;
    let _native = native(&f);
    let sender = editing(&f);
    let reference = issued(&f, &sender).await;
    let mut request = replacement("file");
    let files = crate::discord::outbound_media::prepare(&request.native_attachments)
        .await
        .unwrap();
    request.native_attachments[0].source = cmaeg_core::message::NativeMediaSource::Url {
        url: "https://expired.example.invalid/private-source?expires=1".into(),
    };
    let Admission::Ready(candidate) = f
        .store
        .discord_edit_candidate(&f.channel, &reference, &request)
        .await
        .unwrap()
    else {
        panic!("ready candidate expected")
    };
    // The original bounded download has completed; its source has since expired.
    // The saved operation must resolve before another URL request is attempted.
    let claim = f
        .store
        .begin_discord_media_replacement(
            *candidate,
            &request,
            Replacement::prepare(&request, &files[0]).unwrap(),
        )
        .await
        .unwrap();
    let wire =
        serde_json::to_string(&f.store.load(&claim.prepared.card.id).await.unwrap().0).unwrap();
    assert!(!wire.contains("private-source"));
    let replay = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        update(&sender, &f, &reference, &request),
    )
    .await
    .unwrap();
    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    assert_eq!(patches(&f), 0);
}

#[tokio::test]
async fn discord_media_card_replacement_accepts_new_generated_presentation_for_later_preserving_edits(
) {
    let f = FixtureState::new().await;
    let native = native(&f);
    let sender = editing(&f);
    let reference = issued(&f, &sender).await;
    *native.fault.lock().unwrap() = Some("generated");
    let result = update(&sender, &f, &reference, &replacement("image")).await;
    assert!(
        matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        result.delivery
    );
    let next = result.message_ref.unwrap().open(&f.keys()).unwrap().payload;
    *native.fault.lock().unwrap() = None;
    let edited = update(&sender, &f, &next, &message(true)).await;
    assert!(
        matches!(edited.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        edited.delivery
    );
    assert_eq!(patches(&f), 2);
    assert_eq!(
        native.message.lock().unwrap()["attachments"][0]["width"],
        640
    );
}

#[tokio::test]
async fn discord_media_card_replacement_invalid_multipart_preserves_current_callback_authority() {
    let f = FixtureState::new().await;
    let _native = native(&f);
    let sender = editing(&f);
    let original = issued(&f, &sender).await;
    let mut request = replacement("file");
    request.native_attachments[0].filename = Some("SPOILER_private.bin".into());
    let result = update(&sender, &f, &original, &request).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(patches(&f), 0);
    assert!(
        !f.store
            .load(original.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0
            .superseded
    );
    let valid = update(&sender, &f, &original, &replacement("file")).await;
    assert!(matches!(valid.delivery, DeliveryOutcome::Confirmed { .. }));
    assert_eq!(patches(&f), 1);
}
