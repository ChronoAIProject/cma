use cmaeg_core::{
    message::{NativeMediaKind, OutboundMessage},
    seal::{
        native_media_edit::{NativeMediaDestination, NativeMediaEditAuthority},
        MessageRefPayload,
    },
};
use serde_json::json;
fn message(value: serde_json::Value) -> OutboundMessage {
    serde_json::from_value(value).unwrap()
}
#[test]
fn patch_omission_clear_null_and_exclusivity_are_distinct() {
    let patch = message(json!({"native_media_update":{"caption":""}}));
    patch.validate().unwrap();
    assert_eq!(
        patch
            .native_media_update
            .as_ref()
            .unwrap()
            .caption
            .as_deref(),
        Some("")
    );
    assert!(patch.native_media_update.as_ref().unwrap().text.is_none());
    for value in [
        json!({"native_media_update":null}),
        json!({"native_media_update":{"caption":null}}),
        json!({"native_media_update":{"text":null}}),
        json!({"native_media_update":{"replacement":null}}),
    ] {
        assert!(serde_json::from_value::<OutboundMessage>(value).is_err());
    }
    for value in [
        json!({"native_media_update":{}}),
        json!({"native_media_update":{"caption":"a"},"text":"b"}),
        json!({"native_media_update":{"caption":"a"},"thread_reply":true}),
        json!({"native_media_update":{"text":"a"},"blocks":[{"type":"text","text":"b"}]}),
    ] {
        assert!(message(value).validate().is_err());
    }
    let legacy = message(json!({"text":"legacy"}));
    assert!(serde_json::to_value(&legacy)
        .unwrap()
        .get("native_media_update")
        .is_none());
}
#[test]
fn native_patch_preserves_old_send_caption_bounds_and_rejects_stickers() {
    message(json!({"native_media_update":{"caption":"😀".repeat(512)}}))
        .validate()
        .unwrap();
    assert!(
        message(json!({"native_media_update":{"caption":"😀".repeat(513)}}))
            .validate()
            .is_err()
    );
    assert!(
        message(json!({"native_media_update":{"text":"a".repeat(2001)}}))
            .validate()
            .is_err()
    );
    assert!(message(json!({"native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"caption":""}]})).validate().is_err());
    assert!(message(json!({"native_media_update":{"replacement":{"kind":"sticker","source":{"type":"base64","data":"AQIDBA=="}}}})).validate().is_err());
}
fn reference() -> MessageRefPayload {
    serde_json::from_value(json!({"channel_id":"cmaeg1.ch.fixture","event_id":"event-1","platform_message_id":"800","route_id":"discord:77:99"})).unwrap()
}
fn authority() -> NativeMediaEditAuthority {
    NativeMediaEditAuthority {
        destination: NativeMediaDestination::Discord {
            bot_id: "77".into(),
            channel_id: "99".into(),
        },
        record_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
        original_sha256: "a".repeat(64),
        authority_sha256: "b".repeat(64),
        kind: NativeMediaKind::File,
        revision: 0,
        issued_at: 100,
        expires_at: 200,
    }
}
#[test]
fn historical_effect_authority_does_not_become_edit_authority() {
    let mut reference = reference();
    reference.discord_message = Some(Box::new(cmaeg_core::seal::DiscordMessageDestination {
        bot_id: "77".into(),
        channel_id: "99".into(),
        authority_sha256: [1; 32],
        effects: Some(cmaeg_core::seal::DiscordEffectWindow {
            issued_at: 100,
            expires_at: 200,
        }),
        effects_only: true,
    }));
    reference.validate().unwrap();
    assert!(reference.native_media_edit.is_none());
    reference.native_media_edit = Some(Box::new(authority()));
    reference.validate().unwrap();
    reference.discord_message.as_mut().unwrap().effects_only = false;
    assert!(reference.validate().is_err());
}
#[test]
fn new_authority_refuses_wrong_destinations_controls_expiry_and_relay() {
    for defect in [
        "route",
        "message",
        "window",
        "digest",
        "card",
        "relay",
        "wrong_bot",
        "wrong_kind",
    ] {
        let mut reference = reference();
        reference.native_media_edit = Some(Box::new(authority()));
        reference.validate().unwrap();
        match defect {
            "route" => reference.route_id = "discord:77:100".into(),
            "message" => reference.platform_message_id = "../800".into(),
            "window" => reference.native_media_edit.as_mut().unwrap().expires_at = 86_501,
            "digest" => {
                reference
                    .native_media_edit
                    .as_mut()
                    .unwrap()
                    .original_sha256 = "".into()
            }
            "card" => reference.interaction_record_id = Some("card".into()),
            "relay" => {
                reference.relay_message = Some(cmaeg_core::seal::RelayMessageDestination {
                    platform: cmaeg_core::channel::Platform::Discord,
                    outbound_message_id: "relay".into(),
                })
            }
            "wrong_bot" => {
                reference.native_media_edit.as_mut().unwrap().destination =
                    NativeMediaDestination::Discord {
                        bot_id: "78".into(),
                        channel_id: "99".into(),
                    }
            }
            _ => reference.native_media_edit.as_mut().unwrap().kind = NativeMediaKind::Sticker,
        }
        assert!(reference.validate().is_err(), "{defect}");
    }
}
