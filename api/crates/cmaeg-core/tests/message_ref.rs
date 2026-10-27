use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    ids::MessageRef,
    seal::{open, seal, ChannelPointer, MasterKeyRing, MessageRefPayload, OpenError, TokenKind},
};
fn keys(kid: u8) -> MasterKeyRing {
    MasterKeyRing::parse(&format!("{kid}:{}", STANDARD.encode([kid; 32]))).unwrap()
}
fn payload(keys: &MasterKeyRing) -> MessageRefPayload {
    let channel_id = ChannelPointer {
        v: 1,
        record_id: "record-1".parse().unwrap(),
        record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
    }
    .seal_id(keys)
    .unwrap();
    MessageRefPayload {
        discord_message: None,
        lark_effect: None,
        telegram_effect: None,
        channel_id,
        event_id: "event-1".parse().unwrap(),
        platform_message_id: "platform-message-1".into(),
        route_id: "route-1".into(),
        interaction_record_id: None,
        relay_message: None,
        consumer: None,
    }
}
#[test]
fn stateless_message_refs_round_trip_and_remain_separate_from_event_refs() {
    let keys = keys(1);
    let payload = payload(&keys);
    let reference = MessageRef::seal(&payload, &keys).unwrap();
    assert!(reference.as_str().starts_with("cmaeg1.mr.1."));
    assert_eq!(reference.open(&keys).unwrap().payload, payload);
    assert_eq!(
        serde_json::from_str::<MessageRef>(&serde_json::to_string(&reference).unwrap()).unwrap(),
        reference
    );
    assert!(matches!(
        open::<serde_json::Value>(&keys, TokenKind::EventRef, reference.as_str()),
        Err(OpenError::WrongKind)
    ));
    let reference = MessageRef::new(seal(&keys, TokenKind::EventRef, &payload).unwrap()).unwrap();
    assert!(matches!(reference.open(&keys), Err(OpenError::WrongKind)));
}
#[test]
fn message_refs_enforce_tamper_rotation_payload_and_length_bounds() {
    let old = keys(1);
    let mut payload = payload(&old);
    let reference = MessageRef::seal(&payload, &old).unwrap();
    let both = MasterKeyRing::parse(&format!(
        "2:{},1:{}",
        STANDARD.encode([2; 32]),
        STANDARD.encode([1; 32])
    ))
    .unwrap();
    assert!(reference.open(&both).unwrap().needs_rekey);
    assert!(matches!(
        reference.open(&keys(2)),
        Err(OpenError::UnknownKid(1))
    ));
    let mut corrupted = reference.as_str().to_owned();
    let index = corrupted.len() - 8;
    let replacement = if &corrupted[index..index + 1] == "A" {
        "B"
    } else {
        "A"
    };
    corrupted.replace_range(index..index + 1, replacement);
    assert!(matches!(
        MessageRef::new(corrupted).unwrap().open(&old),
        Err(OpenError::Tampered)
    ));
    payload.platform_message_id.clear();
    assert!(MessageRef::seal(&payload, &old)
        .unwrap()
        .open(&old)
        .is_err());
    assert!(MessageRef::new("plain".repeat(103)).is_err());
    assert!(MessageRef::new(format!("cmaeg1.mr.1.{}", "x".repeat(4096))).is_err());
    assert_eq!(format!("{reference:?}"), "[redacted message ref]");
}

#[test]
fn discord_edit_destination_is_additive_and_cannot_mix_native_or_relay_authority() {
    use cmaeg_core::{
        channel::Platform,
        seal::{DiscordMessageDestination, RelayMessageDestination},
    };
    let keys = keys(1);
    let mut reference = payload(&keys);
    let old = serde_json::to_value(&reference).unwrap();
    assert!(old.get("discord_message").is_none());
    assert_eq!(
        serde_json::from_value::<MessageRefPayload>(old).unwrap(),
        reference
    );
    reference.platform_message_id = "800".into();
    reference.route_id = "discord:77:99".into();
    reference.discord_message = Some(Box::new(DiscordMessageDestination {
        effects_only: false,
        effects: None,
        bot_id: "77".into(),
        channel_id: "99".into(),
        authority_sha256: [3; 32],
    }));
    assert_eq!(
        MessageRef::seal(&reference, &keys)
            .unwrap()
            .open(&keys)
            .unwrap()
            .payload,
        reference
    );
    for variant in 0..6 {
        let mut invalid = reference.clone();
        match variant {
            0 => invalid.route_id = "discord:77:100".into(),
            1 => invalid.platform_message_id = "0800".into(),
            2 => invalid.discord_message.as_mut().unwrap().bot_id = "../me".into(),
            3 => invalid.discord_message.as_mut().unwrap().channel_id = "0".into(),
            4 => invalid.interaction_record_id = Some("card".into()),
            _ => {
                invalid.relay_message = Some(RelayMessageDestination {
                    platform: Platform::Discord,
                    outbound_message_id: "outbound".into(),
                })
            }
        }
        assert!(MessageRef::seal(&invalid, &keys)
            .unwrap()
            .open(&keys)
            .is_err());
    }
}

#[test]
fn discord_effect_windows_are_additive_bounded_and_preserved_by_sealing() {
    use cmaeg_core::seal::{DiscordEffectWindow, DiscordMessageDestination};
    let keys = keys(1);
    let mut message = payload(&keys);
    message.platform_message_id = "800".into();
    message.route_id = "discord:77:99".into();
    message.discord_message = Some(Box::new(DiscordMessageDestination {
        effects_only: false,
        bot_id: "77".into(),
        channel_id: "99".into(),
        authority_sha256: [4; 32],
        effects: None,
    }));
    let legacy = serde_json::to_value(&message).unwrap();
    assert!(legacy["discord_message"].get("effects").is_none());
    assert!(legacy["discord_message"].get("effects_only").is_none());
    assert_eq!(
        serde_json::from_value::<MessageRefPayload>(legacy).unwrap(),
        message
    );
    for (issued_at, expires_at, valid) in [
        (1, 86401, true),
        (1, 2, true),
        (0, 20, false),
        (-1, 20, false),
        (10, 10, false),
        (10, 9, false),
        (1, 86402, false),
    ] {
        message.discord_message.as_mut().unwrap().effects = Some(DiscordEffectWindow {
            issued_at,
            expires_at,
        });
        let reopened = MessageRef::seal(&message, &keys).unwrap().open(&keys);
        assert_eq!(reopened.is_ok(), valid, "{issued_at}..{expires_at}");
        if valid {
            assert_eq!(reopened.unwrap().payload, message);
        }
    }
}

#[test]
fn discord_media_scope_round_trips_and_requires_bounded_effect_authority() {
    use cmaeg_core::seal::{DiscordEffectWindow, DiscordMessageDestination};
    let keys = keys(1);
    let mut message = payload(&keys);
    message.platform_message_id = "800".into();
    message.route_id = "discord:77:99".into();
    message.discord_message = Some(Box::new(DiscordMessageDestination {
        bot_id: "77".into(),
        channel_id: "99".into(),
        authority_sha256: [4; 32],
        effects: Some(DiscordEffectWindow {
            issued_at: 1,
            expires_at: 86401,
        }),
        effects_only: true,
    }));
    assert_eq!(
        serde_json::to_value(&message).unwrap()["discord_message"]["effects_only"],
        true
    );
    assert_eq!(
        MessageRef::seal(&message, &keys)
            .unwrap()
            .open(&keys)
            .unwrap()
            .payload,
        message
    );
    message.discord_message.as_mut().unwrap().effects = None;
    assert!(MessageRef::seal(&message, &keys)
        .unwrap()
        .open(&keys)
        .is_err());
}

#[test]
fn telegram_media_scope_preserves_legacy_serialization_and_cannot_mix_card_authority() {
    use cmaeg_core::seal::TelegramEffectAuthority;
    use serde_json::json;
    let keys = keys(1);
    let channel: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let legacy = json!({
        "record_id":"record-1", "owner":channel["owner"], "provider":channel["provider"],
        "agent_key_hash":vec![3_u8;32], "routing_sha256":"a".repeat(64),
        "bot_id":"77", "chat_id":"-42", "thread_id":"7",
        "admission":{"actor_id":"42","actor_kind":"human","conversation_kind":"group",
            "mentioned":true,"replied_to_bot":false},
        "issued_at":1,"expires_at":86401
    });
    let mut authority: TelegramEffectAuthority = serde_json::from_value(legacy.clone()).unwrap();
    assert!(!authority.effects_only);
    assert_eq!(serde_json::to_value(&authority).unwrap(), legacy);
    authority.effects_only = true;
    let mut reference = payload(&keys);
    reference.platform_message_id = "800".into();
    reference.route_id = "telegram:77:-42".into();
    reference.telegram_effect = Some(authority);
    assert_eq!(
        MessageRef::seal(&reference, &keys)
            .unwrap()
            .open(&keys)
            .unwrap()
            .payload,
        reference
    );
    reference.interaction_record_id = Some("card".into());
    assert!(MessageRef::seal(&reference, &keys)
        .unwrap()
        .open(&keys)
        .is_err());
}
