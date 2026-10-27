use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg_core::{
    channel::{Platform, Secret},
    ids::{ChannelId, ConversationId, EventId, IdempotencyKey, RecordId},
    seal::{
        open, ChannelPointer, EventRef, EventRefPayload, MasterKeyRing, OpenError, ReplyAddress,
        SealError, TokenKind,
    },
};

fn ring() -> MasterKeyRing {
    let one = STANDARD_NO_PAD.encode([1_u8; 32]);
    let two = STANDARD_NO_PAD.encode([2_u8; 32]);
    MasterKeyRing::parse(&format!("2:{two},1:{one}")).expect("key ring")
}

fn old_ring() -> MasterKeyRing {
    let one = STANDARD_NO_PAD.encode([1_u8; 32]);
    MasterKeyRing::parse(&format!("1:{one}")).expect("old key ring")
}

fn channel_id() -> ChannelId {
    ChannelId::new("cmaeg1.ch.pointer").expect("channel token prefix")
}

fn conversation_id() -> ConversationId {
    "conv_00000000000000000000000000000000"
        .parse()
        .expect("conversation id")
}

#[test]
fn channel_pointer_seals_and_opens() {
    let pointer = ChannelPointer {
        v: 1,
        record_url: "https://cma-api.example.test/records/record-1".into(),
        record_id: "record-1".parse::<RecordId>().unwrap(),
    };
    let id = pointer.seal_id(&ring()).expect("channel id");
    let opened = id.open(&ring()).expect("opened pointer");
    assert_eq!(opened.payload.record_id, pointer.record_id);
}

#[test]
fn event_reference_expires_and_redacts_debug() {
    let payload = EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: None,
        channel_id: channel_id(),
        record_id: "record-1".parse().unwrap(),
        event_id: "event-1".parse().unwrap(),
        partition_key: "chat-1".into(),
        conversation_id: conversation_id(),
        expires_at: 100,
        reply: ReplyAddress::NyxidRelay {
            thread_reply: false,
            message_id: "message-1".into(),
            route_id: "route-1".into(),
            platform: Platform::Telegram,
            reply_token: Secret::new("do-not-print"),
            reply_token_expires_at: 200,
        },
        consumer: None,
        lineage: None,
    };
    assert!(!format!("{:?}", payload.reply).contains("do-not-print"));
    let event_ref = EventRef::seal(&payload, &ring()).expect("event ref");
    assert!(matches!(
        event_ref.open(&ring(), 100),
        Err(OpenError::Expired)
    ));
    assert_eq!(event_ref.open(&ring(), 99).unwrap().payload.expires_at, 100);
}

#[test]
fn tampering_wrong_kind_and_unknown_key_are_distinct() {
    let token =
        cmaeg_core::seal::seal(&ring(), TokenKind::Channel, &serde_json::json!({"ok":true}))
            .expect("token");
    assert!(matches!(
        open::<serde_json::Value>(&ring(), TokenKind::EventRef, &token),
        Err(OpenError::WrongKind)
    ));
    let unknown = token.replacen(".2.", ".9.", 1);
    assert!(matches!(
        open::<serde_json::Value>(&ring(), TokenKind::Channel, &unknown),
        Err(OpenError::UnknownKid(9))
    ));
    let mut parts = token.split('.').map(str::to_owned).collect::<Vec<_>>();
    let mut decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&parts[3])
        .unwrap();
    let last = decoded.len() - 1;
    decoded[last] ^= 1;
    parts[3] = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded);
    let tampered = parts.join(".");
    assert!(matches!(
        open::<serde_json::Value>(&ring(), TokenKind::Channel, &tampered),
        Err(OpenError::Tampered)
    ));
}

#[test]
fn old_key_opens_with_rekey_notice() {
    let token = cmaeg_core::seal::seal(
        &old_ring(),
        TokenKind::Channel,
        &serde_json::json!({"ok":true}),
    )
    .expect("old token");
    let opened =
        open::<serde_json::Value>(&ring(), TokenKind::Channel, &token).expect("old token opens");
    assert!(opened.needs_rekey);
}

#[test]
fn oversized_payload_is_refused() {
    let error =
        cmaeg_core::seal::seal(&ring(), TokenKind::Channel, &"x".repeat(4_100)).unwrap_err();
    assert!(matches!(error, SealError::TooLarge));
}

#[test]
fn opaque_blobs_support_large_payloads_and_rekey_notice() {
    let old = old_ring();
    let token = cmaeg_core::seal::seal_blob(&old, &vec![7_u8; 8_000]).expect("blob token");
    let opened = cmaeg_core::seal::open_blob(&ring(), &token).expect("opened blob");
    assert_eq!(opened.payload.len(), 8_000);
    assert_eq!(opened.payload[0], 7);
    assert!(opened.needs_rekey);
    let current = cmaeg_core::seal::seal_blob(&ring(), b"small").expect("current blob");
    assert_eq!(
        cmaeg_core::seal::open_blob(&ring(), &current)
            .unwrap()
            .payload,
        b"small"
    );
}

#[test]
fn conversation_and_idempotency_keys_are_deterministic() {
    let ring = ring();
    let first = ConversationId::derive(&ring, &channel_id(), 1, "chat");
    for _ in 0..1_000 {
        assert_eq!(
            ConversationId::derive(&ring, &channel_id(), 1, "chat"),
            first
        );
    }
    assert_ne!(
        first,
        ConversationId::derive(&ring, &channel_id(), 2, "chat")
    );
    let event: EventId = "event with/unsafe".parse().unwrap();
    let key = IdempotencyKey::from_event(&event);
    assert_eq!(key.as_str(), "evt_event_with_unsafe");
    let long: EventId = "x".repeat(400).parse().unwrap();
    assert_eq!(IdempotencyKey::from_event(&long).as_str().len(), 200);
}

#[test]
fn blob_wire_limit_and_authentication_are_enforced() {
    use cmaeg_core::seal::{open_blob, seal_blob, MAX_BLOB_BYTES};
    let keys =
        MasterKeyRing::parse(&format!("255:{}", STANDARD_NO_PAD.encode([3_u8; 32]))).unwrap();
    let blob = seal_blob(&keys, &vec![7; MAX_BLOB_BYTES]).unwrap();
    assert!(blob.len() <= 65_536);
    assert_eq!(
        open_blob(&keys, &blob).unwrap().payload.len(),
        MAX_BLOB_BYTES
    );
    assert!(matches!(
        seal_blob(&keys, &vec![7; MAX_BLOB_BYTES + 1]),
        Err(SealError::TooLarge)
    ));
    assert!(matches!(
        open_blob(&keys, &vec![b'a'; 65_537]),
        Err(OpenError::Format)
    ));
    let blob = String::from_utf8(seal_blob(&keys, b"secret-fixture").unwrap()).unwrap();
    assert!(open_blob(&keys, blob.replace(".bl.", ".ch.").as_bytes()).is_err());
    assert!(matches!(
        open::<serde_json::Value>(&keys, TokenKind::Channel, &blob),
        Err(OpenError::WrongKind)
    ));
    let mut parts: Vec<_> = blob.split('.').map(str::to_owned).collect();
    let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&parts[3])
        .unwrap();
    bytes[12] ^= 1;
    parts[3] = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    assert!(matches!(
        open_blob(&keys, parts.join(".").as_bytes()),
        Err(OpenError::Tampered)
    ));
}

#[test]
fn record_pointers_allow_cluster_origins_without_relaxing_source_policy() {
    for origin in [
        "http://cma-api.chrono-bot.svc",
        "http://[::1]:8090",
        "https://cma.example",
    ] {
        let pointer = ChannelPointer {
            v: 1,
            record_url: format!("{origin}/records/one"),
            record_id: "one".parse().unwrap(),
        };
        assert!(pointer.validate().is_ok());
    }
    for origin in [
        "http://public.example",
        "https://user:pass@cma.example",
        "https://cma.example/x?y=z",
    ] {
        let pointer = ChannelPointer {
            v: 1,
            record_url: format!("{origin}/records/one"),
            record_id: "one".parse().unwrap(),
        };
        assert!(pointer.validate().is_err());
    }
}

#[test]
fn alternate_key_id_spellings_cannot_alias_authenticated_tokens() {
    let keys = ring();
    let token =
        cmaeg_core::seal::seal(&keys, TokenKind::Channel, &serde_json::json!({"ok":true})).unwrap();
    let blob = String::from_utf8(cmaeg_core::seal::seal_blob(&keys, b"fixture").unwrap()).unwrap();
    for alias in [".02.", ".+2.", ".0002."] {
        assert!(matches!(
            open::<serde_json::Value>(&keys, TokenKind::Channel, &token.replacen(".2.", alias, 1)),
            Err(OpenError::Format)
        ));
        assert!(matches!(
            cmaeg_core::seal::open_blob(&keys, blob.replacen(".2.", alias, 1).as_bytes()),
            Err(OpenError::Format)
        ));
    }
}

#[test]
fn interaction_records_are_bound_to_namespace_id_and_key_ring() {
    use cmaeg_core::seal::{open_blob, open_interaction, seal_interaction};
    let blob = seal_interaction(&old_ring(), "record-a", &vec![7; 32768]).unwrap();
    let opened = open_interaction(&ring(), "record-a", &blob).unwrap();
    assert_eq!(opened.payload, vec![7; 32768]);
    assert!(opened.needs_rekey);
    assert!(open_interaction(&ring(), "record-b", &blob).is_err());
    assert!(open_blob(&ring(), &blob).is_err());
}

#[test]
fn consumer_cards_have_their_own_kind_that_an_interaction_opener_refuses() {
    use cmaeg_core::seal::{
        open_consumer_interaction, open_interaction, seal_consumer_interaction, seal_interaction,
    };
    let blob = seal_consumer_interaction(&old_ring(), "record-a", b"card").unwrap();
    assert!(blob.starts_with(b"cmaeg1.ic.1."));
    let opened = open_consumer_interaction(&ring(), "record-a", &blob).unwrap();
    assert_eq!(opened.payload, b"card");
    assert!(opened.needs_rekey);
    assert!(open_consumer_interaction(&ring(), "record-b", &blob).is_err());
    // An image that only knows `in` cards refuses a consumer's card, and a
    // consumer opener refuses a default card.
    assert!(matches!(
        open_interaction(&ring(), "record-a", &blob),
        Err(OpenError::Format)
    ));
    let default = seal_interaction(&ring(), "record-a", b"card").unwrap();
    assert!(default.starts_with(b"cmaeg1.in."));
    assert!(matches!(
        open_consumer_interaction(&ring(), "record-a", &default),
        Err(OpenError::Format)
    ));
}

#[test]
fn event_reference_retains_every_relay_platform_and_its_activity_binding() {
    let mut activity: cmaeg_core::activity::Activity =
        serde_json::from_str(include_str!("fixtures/activity.json")).expect("activity");
    for platform in [
        Platform::Telegram,
        Platform::Lark,
        Platform::Feishu,
        Platform::Discord,
        Platform::Slack,
        Platform::Whatsapp,
    ] {
        activity.source = cmaeg_core::activity::SourceKind::NyxidRelay {
            platform,
            route_id: "route-1".into(),
        };
        activity.validate().expect("valid activity");
        let payload = EventRefPayload {
            telegram_admission: None,
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel_id(),
            record_id: "record-1".parse().unwrap(),
            event_id: activity.event_id.clone(),
            partition_key: "chat-1".into(),
            conversation_id: conversation_id(),
            expires_at: 100,
            reply: ReplyAddress::NyxidRelay {
                thread_reply: false,
                message_id: "message-1".into(),
                route_id: "route-1".into(),
                platform,
                reply_token: Secret::new("reply-token"),
                reply_token_expires_at: 200,
            },
            consumer: None,
            lineage: None,
        };
        let opened = EventRef::seal(&payload, &ring())
            .expect("event ref")
            .open(&ring(), 99)
            .expect("opened")
            .payload;
        assert_eq!(opened, payload);
        assert!(opened.verifies_activity(&activity));
        let mut other = activity.clone();
        other.source = cmaeg_core::activity::SourceKind::NyxidRelay {
            platform: if platform == Platform::Lark {
                Platform::Feishu
            } else {
                Platform::Lark
            },
            route_id: "route-1".into(),
        };
        assert!(!opened.verifies_activity(&other));
    }
}
