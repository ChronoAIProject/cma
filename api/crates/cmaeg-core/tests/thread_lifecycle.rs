use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    seal::{
        self, EventRefPayload, MasterKeyRing, OpenError, ReplyAddress, ThreadRef, ThreadRefPayload,
        TokenKind,
    },
    thread_lifecycle::{CreateThreadRequest, ManageThreadRequest},
};
use serde_json::json;
fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([4; 32]))).unwrap()
}
#[test]
fn telegram_created_reference_is_separate_chat_scoped_and_never_extends_the_event() {
    use cmaeg_core::seal::TelegramThreadRefPayload;
    let mut event = payload().event;
    event.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "-100".into(),
        message_id: "800".into(),
        thread_id: None,
    };
    let value = TelegramThreadRefPayload {
        v: 2,
        event,
        custody: "b".repeat(64),
        chat_id: "-100".into(),
        thread_id: "901".into(),
        issued_at: 10,
        expires_at: 100,
    };
    let keys = keys();
    let reference = ThreadRef::seal_telegram(&value, &keys).unwrap();
    assert!(reference.open_telegram(&keys, 99).is_ok());
    assert!(matches!(
        reference.open_telegram(&keys, 100),
        Err(OpenError::Expired)
    ));
    assert!(reference.open(&keys, 20).is_err());
    let discord = ThreadRef::seal(&payload(), &keys).unwrap();
    assert!(discord.open_telegram(&keys, 20).is_err());
    for case in 0..8 {
        let mut invalid = value.clone();
        match case {
            0 => invalid.v = 1,
            1 => invalid.chat_id = "-200".into(),
            2 => invalid.thread_id = "1".into(),
            3 => invalid.thread_id = "0901".into(),
            4 => invalid.expires_at += 1,
            5 => invalid.event.activity_sha256 = None,
            6 => invalid.event.reply = payload().event.reply,
            _ => invalid.custody.clear(),
        }
        assert!(!invalid.valid(), "{case}");
        let raw = seal::seal(&keys, TokenKind::ThreadRef, &invalid).unwrap();
        assert!(
            raw.parse::<ThreadRef>()
                .unwrap()
                .open_telegram(&keys, 20)
                .is_err(),
            "{case}"
        );
    }
}

#[test]
fn neutral_create_request_preserves_discord_bytes_and_requires_platform_specific_archive_policy() {
    let discord = r#"{"name":"Review","auto_archive_duration":1440}"#;
    let request: CreateThreadRequest = serde_json::from_str(discord).unwrap();
    assert_eq!(serde_json::to_string(&request).unwrap(), discord);
    request.validate_discord().unwrap();
    assert!(request.validate_telegram().is_err());
    let telegram: CreateThreadRequest = serde_json::from_value(json!({"name":"Review"})).unwrap();
    telegram.validate_telegram().unwrap();
    assert!(telegram.validate_discord().is_err());
    assert_eq!(
        serde_json::to_value(telegram).unwrap(),
        json!({"name":"Review"})
    );
    assert!(serde_json::from_value::<CreateThreadRequest>(
        json!({"name":"Review","auto_archive_duration":null})
    )
    .is_err());
}
fn payload() -> ThreadRefPayload {
    ThreadRefPayload {
        v: 1,
        event: EventRefPayload {
            v: 1,
            telegram_admission: None,
            turn_trace: None,
            activity_sha256: Some("a".repeat(64)),
            channel_id: "cmaeg1.ch.fixture".parse().unwrap(),
            record_id: "record-fixture".parse().unwrap(),
            event_id: "event-fixture".parse().unwrap(),
            partition_key: "partition".into(),
            conversation_id: "conv_00000000000000000000000000000001".parse().unwrap(),
            expires_at: 100,
            reply: ReplyAddress::Discord {
                bot_id: "77".into(),
                channel_id: "98".into(),
                message_id: "700".into(),
                thread_id: None,
            },
            consumer: None,
            lineage: None,
        },
        custody: "b".repeat(64),
        guild_id: "100".into(),
        parent_id: "98".into(),
        thread_id: "700".into(),
        thread_type: 11,
        issued_at: 10,
        expires_at: 100,
    }
}
#[test]
fn lifecycle_reference_preserves_original_expiry_and_never_grants_private_parent_children() {
    let keys = keys();
    let p = payload();
    let reference = ThreadRef::seal(&p, &keys).unwrap();
    assert!(reference.open(&keys, 99).is_ok());
    assert!(matches!(
        reference.open(&keys, 100),
        Err(OpenError::Expired)
    ));
    assert!(!format!("{reference:?}").contains(reference.as_str()));
    for case in 0..6 {
        let mut invalid = p.clone();
        match case {
            0 => invalid.expires_at += 1,
            1 => invalid.thread_type = 12,
            2 => invalid.parent_id = "97".into(),
            3 => invalid.event.activity_sha256 = None,
            4 => invalid.issued_at = 101,
            _ => invalid.thread_id = invalid.parent_id.clone(),
        };
        assert!(!invalid.valid(), "case {case}");
        assert!(matches!(
            ThreadRef::seal(&invalid, &keys),
            Err(seal::SealError::InvalidPayload)
        ));
        let raw = seal::seal(&keys, TokenKind::ThreadRef, &invalid).unwrap();
        assert!(raw.parse::<ThreadRef>().unwrap().open(&keys, 20).is_err());
    }
}
#[test]
fn an_original_private_thread_reference_can_only_name_the_exact_original_thread() {
    let mut p = payload();
    p.thread_type = 12;
    p.thread_id = "99".into();
    p.event.reply = ReplyAddress::Discord {
        bot_id: "77".into(),
        channel_id: "99".into(),
        message_id: "700".into(),
        thread_id: Some("99".into()),
    };
    assert!(p.valid());
    p.thread_id = "88".into();
    assert!(!p.valid());
}
#[test]
fn lifecycle_authority_cannot_be_used_as_a_message_event_or_reply_reference() {
    let keys = keys();
    let payload = payload();
    let reference = ThreadRef::seal(&payload, &keys).unwrap();
    // Native media edits, native Lark cards and all ordinary message effects use mr.
    for kind in [
        TokenKind::MessageRef,
        TokenKind::EventRef,
        TokenKind::ReplyTarget,
    ] {
        assert!(seal::open::<ThreadRefPayload>(&keys, kind, reference.as_str()).is_err());
        let sibling = seal::seal(&keys, kind, &payload).unwrap();
        let incoming: ThreadRef = serde_json::from_value(json!(sibling)).unwrap();
        assert!(incoming.open(&keys, 20).is_err());
    }
}
#[test]
fn lifecycle_requests_reject_destinations_other_members_and_unsupported_archive_durations() {
    for duration in [60, 1440, 4320, 10080] {
        assert!(CreateThreadRequest {
            name: "Review".into(),
            auto_archive_duration: Some(duration)
        }
        .validate()
        .is_ok());
    }
    for name in [
        "".to_owned(),
        " ".to_owned(),
        "a".repeat(101),
        "bad\u{1b}name".into(),
    ] {
        assert!(CreateThreadRequest {
            name,
            auto_archive_duration: Some(1440)
        }
        .validate()
        .is_err());
    }
    assert!(CreateThreadRequest {
        name: "Review".into(),
        auto_archive_duration: Some(0)
    }
    .validate()
    .is_err());
    for value in [
        json!({"target":{"type":"thread","thread_id":"99"},"action":"archive"}),
        json!({"target":{"type":"event","event_ref":"cmaeg1.ev.fixture"},"action":"join","user_id":"51"}),
        json!({"target":{"type":"event","event_ref":"cmaeg1.ev.fixture"},"action":"delete"}),
    ] {
        assert!(serde_json::from_value::<ManageThreadRequest>(value).is_err());
    }
}

#[test]
fn rename_requires_an_explicit_bounded_name_and_legacy_requests_keep_their_wire_shape() {
    let target = json!({"type":"event","event_ref":"cmaeg1.ev.fixture"});
    for action in ["archive", "reopen", "join", "leave"] {
        let value = json!({"target":target,"action":action});
        let request: ManageThreadRequest = serde_json::from_value(value.clone()).unwrap();
        assert!(request.validate().is_ok());
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            format!(
                r#"{{"target":{{"type":"event","event_ref":"cmaeg1.ev.fixture"}},"action":"{action}"}}"#
            )
        );
        assert_eq!(serde_json::to_value(request).unwrap(), value);
        let request: ManageThreadRequest =
            serde_json::from_value(json!({"target":target,"action":action,"name":"Review"}))
                .unwrap();
        assert!(request.validate().is_err());
        assert!(serde_json::from_value::<ManageThreadRequest>(
            json!({"target":target,"action":action,"name":null})
        )
        .is_err());
    }
    for name in ["a".into(), "🧵".repeat(100)] {
        let request: ManageThreadRequest =
            serde_json::from_value(json!({"target":target,"action":"rename","name":name})).unwrap();
        assert!(request.validate().is_ok());
    }
    for name in [
        "".into(),
        " \t".into(),
        "🧵".repeat(101),
        "bad\u{1b}name".into(),
    ] {
        let request: ManageThreadRequest =
            serde_json::from_value(json!({"target":target,"action":"rename","name":name})).unwrap();
        assert!(request.validate().is_err());
    }
    let missing: ManageThreadRequest =
        serde_json::from_value(json!({"target":target,"action":"rename"})).unwrap();
    assert!(missing.validate().is_err());
    assert!(serde_json::from_value::<ManageThreadRequest>(
        json!({"target":target,"action":"rename","name":null})
    )
    .is_err());
}
