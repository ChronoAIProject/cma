//! A real native HTTP fixture proves same-file revisions and uncertain-write fencing.
use super::{
    discord_card_edit_tests::{patches, update},
    discord_controls_tests::sender,
    discord_media_controls_tests::{media, send},
    intents::DeliveryOutcome,
    send::Sender,
};
use crate::discord::components_fixture::{message, FixtureState};
use axum::http::StatusCode;
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

struct Native {
    message: Arc<Mutex<Value>>,
    corrupt_patch: Arc<Mutex<Option<&'static str>>>,
    corrupt_send: Arc<Mutex<Option<&'static str>>>,
}
fn native(f: &FixtureState) -> Native {
    let current = Arc::new(Mutex::new(Value::Null));
    let corrupt_patch = Arc::new(Mutex::new(None));
    let corrupt_send = Arc::new(Mutex::new(None));
    let state = Native {
        message: current.clone(),
        corrupt_patch: corrupt_patch.clone(),
        corrupt_send: corrupt_send.clone(),
    };
    let memory = Arc::downgrade(&f.rest.memory);
    let metadata = if f.thread_kind.is_some() {
        f.thread_metadata()
    } else {
        json!({"id":"99","type":1})
    };
    *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
        let mut status = StatusCode::OK;
        let mut response = if call.method == "POST" && call.path == "/api/v10/channels/99/messages"
        {
            let wire = String::from_utf8_lossy(&call.body);
            let mut body: Value = serde_json::from_str(
                wire.split("name=\"payload_json\"\r\n\r\n")
                    .nth(1)
                    .unwrap()
                    .split("\r\n--")
                    .next()
                    .unwrap(),
            )
            .unwrap();
            body["id"] = "800".into();
            body["channel_id"] = "99".into();
            body["author"] = json!({"id":"77","bot":true});
            body["type"] = 19.into();
            body["attachments"] = json!([{"id":"900","filename":"report.bin","content_type":"application/octet-stream","size":4,"description":"Review this upload"}]);
            *current.lock().unwrap() = body.clone();
            if let Some(field) = *corrupt_send.lock().unwrap() {
                corrupt(&mut body, field);
            }
            body
        } else if call.method == "PATCH" {
            let patch: Value = serde_json::from_slice(&call.body).unwrap();
            assert!(
                patch.get("attachments").is_none(),
                "preserve by omission, never clear files"
            );
            assert!(patch.get("embeds").is_none());
            assert_eq!(
                patch["allowed_mentions"],
                json!({"parse":[],"replied_user":false})
            );
            let mut body = current.lock().unwrap();
            body["content"] = patch["content"].clone();
            body["components"] = patch["components"].clone();
            let mut response = body.clone();
            if let Some(field) = *corrupt_patch.lock().unwrap() {
                if field == "500" {
                    status = StatusCode::INTERNAL_SERVER_ERROR;
                } else {
                    corrupt(&mut response, field);
                }
            }
            response
        } else if call.method == "POST" {
            status = StatusCode::NO_CONTENT;
            Value::Null
        } else if call.path.ends_with("/messages/800") {
            current.lock().unwrap().clone()
        } else {
            metadata.clone()
        };
        if status != StatusCode::OK {
            response = json!({});
        }
        memory
            .upgrade()
            .unwrap()
            .responses
            .lock()
            .unwrap()
            .push_back((status, response));
    }));
    state
}
fn corrupt(value: &mut Value, field: &str) {
    match field {
        "id" => value["attachments"][0]["id"] = "901".into(),
        "size" => value["attachments"][0]["size"] = 5.into(),
        "filename" => value["attachments"][0]["filename"] = "other.bin".into(),
        "mime" => value["attachments"][0]["content_type"] = "text/plain".into(),
        "caption" => value["attachments"][0]["description"] = "changed".into(),
        "width" => value["attachments"][0]["width"] = 1.into(),
        "missing" => {
            value.as_object_mut().unwrap().remove("attachments");
        }
        "controls" => value["components"] = json!([]),
        "content" => value["content"] = "changed".into(),
        _ => panic!("unknown mutation"),
    }
}
fn editing(f: &FixtureState) -> Sender {
    let mut sender = sender(f, true);
    sender.relay = sender
        .relay
        .with_discord_media_controls(true)
        .with_discord_media_card_edits(true)
        .with_discord_card_edits(true)
        .with_consumer_interactions(true);
    sender
}
async fn issued(f: &FixtureState, sender: &Sender) -> MessageRefPayload {
    let receipt = send(f, sender, &media(false, "file", &[1, 2, 3, 4]), true).await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        receipt.delivery
    );
    receipt
        .message_ref
        .unwrap()
        .open(&f.keys())
        .unwrap()
        .payload
}
fn text_only() -> OutboundMessage {
    serde_json::from_value(json!({"blocks":[{"type":"text","text":"Review complete"}]})).unwrap()
}

#[tokio::test]
async fn discord_media_card_edits_preserve_exact_upload_through_revisions_and_gate_off_replay() {
    for scope in ["default", "named", "thread"] {
        let f = match scope {
            "named" => FixtureState::named().await,
            "thread" => FixtureState::named_thread(11).await,
            _ => FixtureState::new().await,
        };
        let native = native(&f);
        let mut sender = editing(&f);
        let original = issued(&f, &sender).await;
        let initial = native.message.lock().unwrap().clone();
        let original_card = f
            .store
            .load(original.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        let identity = serde_json::to_value(&original_card.discord).unwrap();
        assert_eq!(identity["media"]["confirmed"]["attachment_id"], "900");
        let receipt = update(&sender, &f, &original, &message(true)).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            receipt.delivery
        );
        let next_ref = receipt
            .message_ref
            .as_ref()
            .unwrap()
            .open(&f.keys())
            .unwrap()
            .payload;
        let next = f
            .store
            .load(next_ref.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0;
        assert_eq!(next.revision, 2);
        assert_eq!(serde_json::to_value(&next.discord).unwrap(), identity);
        assert_eq!(
            native.message.lock().unwrap()["attachments"],
            initial["attachments"]
        );
        sender.relay = sender
            .relay
            .with_discord_media_card_edits(false)
            .with_discord_card_edits(false)
            .with_discord_media_controls(false)
            .with_consumer_interactions(false);
        let replay = update(&sender, &f, &original, &message(true)).await;
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&receipt).unwrap()
        );
        assert_eq!(patches(&f), 1);
        let removed = update(&sender, &f, &next_ref, &text_only()).await;
        assert!(matches!(
            removed.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert_eq!(patches(&f), 2);
        assert_eq!(native.message.lock().unwrap()["components"], json!([]));
        assert_eq!(
            native.message.lock().unwrap()["attachments"],
            initial["attachments"]
        );
    }
}

#[tokio::test]
async fn discord_media_card_send_requires_exact_file_and_control_echo_before_binding() {
    for field in [
        "size", "filename", "mime", "caption", "missing", "controls", "content",
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let sender = editing(&f);
        *native.corrupt_send.lock().unwrap() = Some(field);
        let result = send(&f, &sender, &media(false, "file", &[1, 2, 3, 4]), true).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unknown { .. }),
            "{field}"
        );
        assert!(result.message_ref.is_none());
        assert_eq!(patches(&f), 0);
    }
}

#[tokio::test]
async fn discord_media_card_pre_read_refuses_changed_upload_without_superseding() {
    for field in [
        "id", "size", "filename", "mime", "caption", "width", "missing", "controls", "content",
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let sender = editing(&f);
        let reference = issued(&f, &sender).await;
        corrupt(&mut native.message.lock().unwrap(), field);
        let result = update(&sender, &f, &reference, &message(true)).await;
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
async fn discord_media_card_bad_patch_receipt_is_uncertain_and_never_resent() {
    for field in [
        "id", "size", "filename", "mime", "caption", "width", "missing", "controls", "content",
        "500",
    ] {
        let f = FixtureState::new().await;
        let native = native(&f);
        let sender = editing(&f);
        let reference = issued(&f, &sender).await;
        *native.corrupt_patch.lock().unwrap() = Some(field);
        let result = update(&sender, &f, &reference, &message(true)).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unknown { .. }),
            "{field}: {:?}",
            result.delivery
        );
        assert!(result.message_ref.is_none());
        assert_eq!(patches(&f), 1);
        let replay = update(&sender, &f, &reference, &message(true)).await;
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
        assert_eq!(patches(&f), 1);
    }
}

#[tokio::test]
async fn discord_media_card_initial_edit_gate_and_current_source_authority_are_required() {
    for disabled in [false, true] {
        let mut f = FixtureState::named().await;
        let _native = native(&f);
        let mut sender = editing(&f);
        let reference = issued(&f, &sender).await;
        if disabled {
            sender.relay = sender.relay.with_discord_media_card_edits(false);
        } else {
            Arc::make_mut(&mut f.channel)
                .definition
                .secrets
                .discord_bot_token = None;
        }
        let result = update(&sender, &f, &reference, &message(true)).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unsupported));
        assert_eq!(patches(&f), 0);
    }
}

#[tokio::test]
async fn discord_media_card_edited_controls_keep_original_callback_authority() {
    let f = FixtureState::named().await;
    let native = native(&f);
    let sender = editing(&f);
    let original = issued(&f, &sender).await;
    let original_custom = native.message.lock().unwrap()["components"][0]["components"][0]
        ["custom_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = update(&sender, &f, &original, &message(true)).await;
    let next = result.message_ref.unwrap().open(&f.keys()).unwrap().payload;
    let new_custom = native.message.lock().unwrap()["components"][0]["components"][0]["custom_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = f.receive(f.callback(&original_custom), 2).await;
    assert!(f
        .store
        .load(original.interaction_record_id.as_ref().unwrap())
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    let mut click = f.callback(&new_custom);
    click["data"]["component_type"] = 3.into();
    click["data"]["values"] = json!(["1"]);
    f.receive(click, 2).await.unwrap();
    let card = f
        .store
        .load(next.interaction_record_id.as_ref().unwrap())
        .await
        .unwrap()
        .0;
    assert!(card.consumed.is_some());
    assert_eq!(card.consumer, f.original.consumer);
    assert_eq!(
        card.discord.as_ref().unwrap().conversation_id,
        f.original.conversation_id
    );
    assert!(card
        .discord
        .as_ref()
        .unwrap()
        .media
        .as_ref()
        .unwrap()
        .has_edit_authority());
    assert_eq!(patches(&f), 1);
}

#[tokio::test]
async fn discord_media_card_edits_refuse_new_uploads_and_form_combinations_before_writes() {
    let f = FixtureState::new().await;
    let _native = native(&f);
    let mut sender = editing(&f);
    sender.relay = sender.relay.with_discord_forms(true);
    let reference = issued(&f, &sender).await;
    let mut mixed = message(false);
    mixed
        .blocks
        .extend(crate::discord::modal_fixture::message().blocks);
    for request in [
        media(true, "file", &[4, 3, 2, 1]),
        crate::discord::modal_fixture::message(),
        mixed.clone(),
    ] {
        let result = update(&sender, &f, &reference, &request).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unsupported));
        assert_eq!(patches(&f), 0);
    }
    mixed.native_attachments = media(false, "file", &[1, 2, 3, 4]).native_attachments;
    let calls = f.rest.memory.calls.lock().unwrap().len();
    let mut form_media = crate::discord::modal_fixture::message();
    form_media.native_attachments = mixed.native_attachments.clone();
    for request in [mixed, form_media] {
        let receipt = send(&f, &sender, &request, true).await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), calls);
    }
    assert!(
        !f.store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0
            .superseded
    );
}
