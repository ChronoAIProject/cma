//! New authority stays disjoint from legacy and relay message references.
use base64::Engine as _;
use cmaeg_core::{
    channel::{LarkRegion, NativeLarkIdentity, NativeLarkReply},
    ids::MessageRef,
    seal::{
        native_lark_effect::{NativeLarkEffectAuthority, NativeLarkEffectTarget},
        received_reaction::{ReceivedReactionAuthority, ReceivedReactionPlatform},
        MasterKeyRing, MessageRefPayload,
    },
};

fn payload() -> MessageRefPayload {
    let a = NativeLarkEffectAuthority {
        reply: NativeLarkReply {
            mentioned: false,
            identity: NativeLarkIdentity {
                region: LarkRegion::Feishu,
                app_id: "cli_native".into(),
                tenant_key: "tenant".into(),
                bot_open_id: "ou_bot".into(),
            },
            chat_id: "oc_chat".into(),
            message_id: "om_message".into(),
            thread_id: None,
            revision: "a".repeat(64),
        },
        record_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
        target: NativeLarkEffectTarget::Received {
            sender_id: "ou_human".into(),
        },
        original_sha256: "b".repeat(64),
        authority_sha256: "c".repeat(64),
        observed_at: 100,
        expires_at: 200,
    };
    MessageRefPayload {
        route_id: a.route(),
        received_reaction: None,
        native_lark_effect: Some(Box::new(a)),
        channel_id: "cmaeg1.ch.fixture".parse().unwrap(),
        event_id: "event-native".parse().unwrap(),
        platform_message_id: "om_message".into(),
        consumer: None,
        whatsapp_reaction: None,
        telegram_effect: None,
        discord_message: None,
        lark_effect: None,
        relay_message: None,
        interaction_record_id: None,
    }
}
#[test]
fn native_lark_effect_authority_round_trips_without_relay_authority() {
    let payload = payload();
    payload.validate().unwrap();
    let keys = MasterKeyRing::parse(&format!(
        "1:{}",
        base64::engine::general_purpose::STANDARD.encode([3; 32])
    ))
    .unwrap();
    let seal = MessageRef::seal(&payload, &keys).unwrap();
    assert_eq!(seal.open(&keys).unwrap().payload, payload);
    let value = serde_json::to_value(&payload).unwrap();
    assert!(value.get("lark_effect").is_none());
    assert!(value.get("relay_message").is_none());
}
#[test]
fn native_lark_effect_authority_rejects_mixed_destinations_and_bad_lifetimes() {
    for defect in [
        "route",
        "message",
        "sender",
        "ttl",
        "receipt",
        "relay",
        "interaction",
    ] {
        let mut payload = payload();
        match defect {
            "route" => payload.route_id = "relay-route".into(),
            "message" => payload.platform_message_id = "om_other".into(),
            "sender" => {
                payload.native_lark_effect.as_mut().unwrap().target =
                    NativeLarkEffectTarget::Received {
                        sender_id: "ou_bot".into(),
                    }
            }
            "ttl" => payload.native_lark_effect.as_mut().unwrap().expires_at = 100 + 86_401,
            "receipt" => {
                payload.native_lark_effect.as_mut().unwrap().original_sha256 = "bad".into()
            }
            "relay" => {
                payload.relay_message = Some(cmaeg_core::seal::RelayMessageDestination {
                    platform: cmaeg_core::channel::Platform::Feishu,
                    outbound_message_id: "relay-send".into(),
                })
            }
            _ => payload.interaction_record_id = Some("card".into()),
        }
        assert!(payload.validate().is_err(), "{defect}");
    }
}

#[test]
fn received_native_authorities_cannot_be_combined_in_a_message_seal() {
    let keys = MasterKeyRing::parse(&format!(
        "1:{}",
        base64::engine::general_purpose::STANDARD.encode([3; 32])
    ))
    .unwrap();
    let lark = payload();
    let native_lark = lark.native_lark_effect.as_ref().unwrap().clone();
    for platform in [
        ReceivedReactionPlatform::Telegram,
        ReceivedReactionPlatform::Discord,
    ] {
        let received = ReceivedReactionAuthority {
            platform,
            bot_id: "77".into(),
            chat_id: "99".into(),
            actor_id: "42".into(),
            thread_id: None,
            event_sha256: "a".repeat(64),
            authority_sha256: "b".repeat(64),
            received_at: 100,
            expires_at: 200,
        };
        let mut message = lark.clone();
        message.native_lark_effect = None;
        message.route_id = received.route();
        message.platform_message_id = "800".into();
        message.received_reaction = Some(Box::new(received.clone()));
        message.validate().unwrap();
        MessageRef::seal(&message, &keys).unwrap();
        // Each validator rejects the sibling even with its own valid route and IDs.
        message.native_lark_effect = Some(native_lark.clone());
        assert!(received.validate(&message).is_err());
        assert!(MessageRef::seal(&message, &keys)
            .unwrap()
            .open(&keys)
            .is_err());
        let mut message = lark.clone();
        message.received_reaction = Some(Box::new(received));
        assert!(native_lark.validate(&message).is_err());
        assert!(MessageRef::seal(&message, &keys)
            .unwrap()
            .open(&keys)
            .is_err());
    }
}
