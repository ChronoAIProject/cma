use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::Activity,
    seal::{
        ChannelPointer, EventRefPayload, MasterKeyRing, OpenError, ReplyAddress,
        ReplyTargetPayload, ReplyTargetRef, TokenKind,
    },
};

fn keys(kid: u8) -> MasterKeyRing {
    MasterKeyRing::parse(&format!("{kid}:{}", STANDARD.encode([kid; 32]))).unwrap()
}
fn payload(keys: &MasterKeyRing) -> ReplyTargetPayload {
    let activity: Activity = serde_json::from_str(include_str!("fixtures/activity.json")).unwrap();
    let channel_id = ChannelPointer {
        v: 1,
        record_id: "record-1".parse().unwrap(),
        record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
    }
    .seal_id(keys)
    .unwrap();
    ReplyTargetPayload {
        v: 1,
        provider: "cma_codex".parse().unwrap(),
        binding_id: "binding-1".parse().unwrap(),
        agent_key_hash: [2; 32],
        issued_at: 100,
        expires_at: 86500,
        event: EventRefPayload {
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel_id.clone(),
            record_id: "record-1".parse().unwrap(),
            event_id: activity.event_id,
            partition_key: "chat-1".into(),
            conversation_id: cmaeg_core::ids::ConversationId::derive(
                keys,
                &channel_id,
                1,
                "chat-1",
            ),
            expires_at: 200,
            reply: ReplyAddress::NyxidRelay {
                message_id: "original".into(),
                route_id: "route-1".into(),
                platform: cmaeg_core::channel::Platform::Telegram,
                reply_token: cmaeg_core::channel::Secret::new(""),
                reply_token_expires_at: 1,
            },
        },
    }
}
#[test]
fn target_is_separate_from_event_and_interaction_authority_and_survives_key_rotation() {
    let ring = keys(1);
    let target = ReplyTargetRef::seal(&payload(&ring), &ring).unwrap();
    assert!(target.as_str().len() <= 4096);
    assert_eq!(format!("{target:?}"), "[redacted reply target]");
    assert_eq!(
        target
            .open(&ring, 300)
            .unwrap()
            .payload
            .event
            .event_id
            .as_str(),
        "msg-1"
    );
    for kind in [
        TokenKind::EventRef,
        TokenKind::MessageRef,
        TokenKind::Interaction,
    ] {
        assert!(matches!(
            cmaeg_core::seal::open::<serde_json::Value>(&ring, kind, target.as_str()),
            Err(OpenError::WrongKind)
        ));
    }
    let both = MasterKeyRing::parse(&format!(
        "2:{},1:{}",
        STANDARD.encode([2; 32]),
        STANDARD.encode([1; 32])
    ))
    .unwrap();
    assert!(target.open(&both, 300).unwrap().needs_rekey);
    assert!(matches!(
        target.open(&keys(2), 300),
        Err(OpenError::UnknownKid(1))
    ));
    assert!(matches!(target.open(&ring, 86500), Err(OpenError::Expired)));
    assert!(matches!(target.open(&ring, 99), Err(OpenError::Format)));
}
#[test]
fn target_refuses_unverified_context_and_issuance_after_original_event_expiry() {
    let ring = keys(1);
    for case in ["unverified", "late", "overlong", "custom"] {
        let mut value = payload(&ring);
        match case {
            "unverified" => value.event.activity_sha256 = None,
            "late" => value.issued_at = value.event.expires_at,
            "overlong" => value.expires_at += 1,
            "custom" => value.event.reply = ReplyAddress::None,
            _ => unreachable!(),
        }
        assert!(
            ReplyTargetRef::seal(&value, &ring)
                .unwrap()
                .open(&ring, 300)
                .is_err(),
            "{case}"
        );
    }
}
