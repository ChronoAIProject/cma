use super::{
    discord_controls_tests::sender,
    gate::recording::RecordingGate,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::discord::{
    components_fixture::{message, FixtureState},
    InboxLaunch, InboxReader,
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::message::OutboundMessage;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn media(select: bool, kind: &str, bytes: &[u8]) -> OutboundMessage {
    let mut body = serde_json::to_value(message(select)).unwrap();
    body["native_attachments"] = json!([{"kind":kind,"source":{"type":"base64","data":STANDARD.encode(bytes)},"filename":"report.bin","mime_type":"application/octet-stream","caption":"Review this upload"}]);
    serde_json::from_value(body).unwrap()
}
fn ready(f: &FixtureState) {
    f.rest
        .memory
        .responses
        .lock()
        .unwrap()
        .push_back((StatusCode::OK, json!({"id":"99","type":1})));
    f.rest.respond(
        StatusCode::OK,
        json!({"id":"800","channel_id":"99","author":{"id":"77","bot":true}}),
    );
}
pub(super) async fn send(
    f: &FixtureState,
    sender: &Sender,
    message: &OutboundMessage,
    allowed: bool,
) -> IntentReceipt {
    let trace = tracing::Span::none();
    let gate = RecordingGate::new(allowed);
    sender
        .reply(
            &SendContext {
                trace: &trace,
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            },
            &f.original,
            message,
        )
        .await
        .0
}
fn payload(f: &FixtureState) -> Value {
    let calls = f.rest.memory.calls.lock().unwrap();
    let sent = calls.last().unwrap();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/api/v10/channels/99/messages");
    assert!(sent
        .content_type
        .as_ref()
        .unwrap()
        .starts_with("multipart/form-data;"));
    let wire = String::from_utf8_lossy(&sent.body);
    assert_eq!(wire.matches("name=\"files[0]\"").count(), 1);
    assert!(!wire.contains("name=\"files[1]\""));
    assert!(!wire.contains("private-yes"));
    serde_json::from_str(
        wire.split("name=\"payload_json\"\r\n\r\n")
            .nth(1)
            .unwrap()
            .split("\r\n--")
            .next()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn discord_media_controls_sends_one_multipart_and_retains_compact_same_destination_card() {
    for kind in ["image", "file", "audio", "video"] {
        for select in [false, true] {
            let f = FixtureState::new().await;
            ready(&f);
            let mut sender = sender(&f, true);
            sender.relay = sender.relay.with_discord_media_controls(true);
            // This source alone exceeds the encrypted record bound if retained.
            let bytes = vec![9; 64 * 1024];
            let receipt = send(&f, &sender, &media(select, kind, &bytes), true).await;
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
            let body = payload(&f);
            assert_eq!(
                body["components"][0]["components"][0]["type"],
                if select { 3 } else { 2 }
            );
            assert_eq!(
                body["attachments"],
                json!([{"id":0,"filename":"report.bin","description":"Review this upload"}])
            );
            assert_eq!(
                body["allowed_mentions"],
                json!({"parse":[],"replied_user":false})
            );
            assert_eq!(body["message_reference"]["channel_id"], "99");
            let reference = receipt
                .message_ref
                .unwrap()
                .open(&f.keys())
                .unwrap()
                .payload;
            assert_eq!(reference.platform_message_id, "800");
            assert!(reference.native_media_edit.is_none());
            let (card, _) = f
                .store
                .load(reference.interaction_record_id.as_deref().unwrap())
                .await
                .unwrap();
            assert_eq!(card.destination.as_ref().unwrap().platform_id, "800");
            assert!(card.message.native_attachments.is_empty());
            let saved = serde_json::to_value(&card).unwrap();
            assert_eq!(saved["discord"]["media"]["kind"], kind);
            assert_eq!(saved["discord"]["media"]["bytes"], bytes.len());
            assert_eq!(
                saved["discord"]["media"]["sha256"],
                format!("{:x}", Sha256::digest(&bytes))
            );
            assert!(!saved.to_string().contains(&STANDARD.encode(&bytes)));
            assert!(saved.to_string().len() < 16 * 1024);
        }
    }
}

#[tokio::test]
async fn discord_media_controls_default_off_and_single_upload_bounds_stop_new_issuance() {
    for enabled in [false, true] {
        let f = FixtureState::new().await;
        let mut sender = sender(&f, true);
        assert!(!sender.relay.discord_media_controls_enabled());
        sender.relay = sender.relay.with_discord_media_controls(enabled);
        let mut outbound = media(false, "file", &[1, 2, 3, 4]);
        if enabled {
            outbound
                .native_attachments
                .push(outbound.native_attachments[0].clone());
        }
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let receipt = send(&f, &sender, &outbound, true).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
        assert!(receipt.message_ref.is_none());
        assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
        assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        assert!(f.rest.memory.identities.lock().unwrap().is_empty());
        if !enabled {
            assert!(f.provider.memory.lock().unwrap().seen.is_empty());
        }
    }
}

#[tokio::test]
async fn discord_media_controls_never_bind_or_split_when_commit_denied_or_send_is_uncertain() {
    for allowed in [false, true] {
        let f = FixtureState::new().await;
        ready(&f);
        f.rest.respond(StatusCode::INTERNAL_SERVER_ERROR, json!({}));
        let mut sender = sender(&f, true);
        sender.relay = sender.relay.with_discord_media_controls(true);
        let before: std::collections::BTreeSet<_> = f
            .setup
            .fixture
            .memory
            .rows
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let receipt = send(&f, &sender, &media(false, "image", &[1, 2, 3, 4]), allowed).await;
        assert!(receipt.message_ref.is_none());
        if allowed {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
            assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
            let _ = payload(&f);
            let ids: Vec<_> = f
                .setup
                .fixture
                .memory
                .rows
                .lock()
                .unwrap()
                .keys()
                .filter(|id| !before.contains(*id))
                .cloned()
                .collect();
            assert_eq!(ids.len(), 1);
            let card = f.store.load(&ids[0]).await.unwrap().0;
            assert!(card.destination.is_none());
            assert!(card.reference.is_none());
            assert!(card.consumed.is_none());
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
            assert_eq!(
                before.len(),
                f.setup.fixture.memory.rows.lock().unwrap().len()
            );
            assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn discord_media_controls_callback_restores_same_consumer_after_issuance_disabled() {
    for select in [false, true] {
        let f = FixtureState::named().await;
        ready(&f);
        let mut sender = sender(&f, true);
        sender.relay = sender
            .relay
            .with_discord_media_controls(true)
            .with_consumer_interactions(true);
        let receipt = send(&f, &sender, &media(select, "image", &[1, 2, 3, 4]), true).await;
        let reference = receipt
            .message_ref
            .unwrap()
            .open(&f.keys())
            .unwrap()
            .payload;
        let card_id = reference.interaction_record_id.unwrap();
        let body = payload(&f);
        sender.relay = sender.relay.with_discord_media_controls(false);
        assert!(matches!(
            send(&f, &sender, &media(select, "image", &[1, 2, 3, 4]), true)
                .await
                .delivery,
            DeliveryOutcome::Unsupported
        ));
        let mut raw = f.callback(
            body["components"][0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        if select {
            raw["data"]["component_type"] = 3.into();
            raw["data"]["values"] = json!(["1"]);
        }
        let id = f.id(&raw);
        f.setup.fixture.fail(&id, false);
        assert!(f.receive(raw.clone(), 2).await.is_err());
        assert!(f.store.load(&card_id).await.unwrap().0.consumed.is_some());
        f.setup.fixture.advance(3000);
        f.receive(raw, 2).await.unwrap();
        let InboxLaunch::Launch(verified) = InboxReader::new(f.store.channel_records(), f.keys())
            .load(&id, f.channel.clone(), f.setup.now())
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(verified.request.conversation_id, f.original.conversation_id);
        assert_eq!(verified.request.consumer, f.original.consumer);
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
        let retained = f.store.load(&card_id).await.unwrap().0;
        assert_eq!(retained.destination.unwrap().platform_id, "800");
        assert!(retained.discord.unwrap().media.is_some());
    }
}

#[tokio::test]
async fn discord_media_controls_cannot_be_edited_as_text_or_rebound_to_another_destination() {
    for field in ["message", "actor", "bot", "channel"] {
        let f = FixtureState::new().await;
        ready(&f);
        let mut sender = sender(&f, true);
        sender.relay = sender
            .relay
            .with_discord_media_controls(true)
            .with_discord_card_edits(true);
        let receipt = send(&f, &sender, &media(false, "image", &[1, 2, 3, 4]), true).await;
        let reference = receipt
            .message_ref
            .unwrap()
            .open(&f.keys())
            .unwrap()
            .payload;
        assert!(f
            .store
            .discord_edit_candidate(&f.channel, &reference, &message(false))
            .await
            .is_err());
        let body = payload(&f);
        let mut raw = f.callback(
            body["components"][0]["components"][0]["custom_id"]
                .as_str()
                .unwrap(),
        );
        match field {
            "message" => raw["message"]["id"] = "801".into(),
            "actor" => raw["user"]["id"] = "43".into(),
            "bot" => raw["message"]["author"]["id"] = "78".into(),
            _ => raw["channel_id"] = "100".into(),
        }
        let _ = f.receive(raw, 2).await;
        assert!(f
            .store
            .load(reference.interaction_record_id.as_deref().unwrap())
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn discord_media_controls_source_revocation_and_named_gate_deny_before_upload() {
    for named_allowed in [false, true] {
        let mut f = FixtureState::named().await;
        let mut sender = sender(&f, true);
        sender.relay = sender
            .relay
            .with_discord_media_controls(true)
            .with_consumer_interactions(named_allowed);
        if named_allowed {
            std::sync::Arc::make_mut(&mut f.channel)
                .definition
                .secrets
                .discord_bot_token = None;
        }
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let receipt = send(&f, &sender, &media(false, "file", &[1, 2, 3, 4]), true).await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Unsupported | DeliveryOutcome::Rejected { .. }
        ));
        assert!(receipt.message_ref.is_none());
        assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
        assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        assert!(f.rest.memory.identities.lock().unwrap().is_empty());
    }
}
