use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::Source,
    conversation_history::HISTORY_PAGE_SIZE,
    seal::{
        self, HistoryCursor, HistoryCursorPayload, HistoryOrigin, MasterKeyRing, OpenError,
        TokenKind,
    },
};
use serde_json::json;

fn payload() -> HistoryCursorPayload {
    HistoryCursorPayload {
        v: 1,
        event_sha256: "a".repeat(64),
        custody: "b".repeat(64),
        origin: HistoryOrigin {
            channel_id: "99".into(),
            channel_type: 12,
            guild_id: Some("100".into()),
            parent_id: Some("98".into()),
        },
        anchor_message_id: "18446744073709551615".into(),
        before_message_id: "18446744073709551614".into(),
        page_size: HISTORY_PAGE_SIZE,
        issued_at: 10,
        expires_at: 100,
    }
}
#[test]
fn cursor_roundtrip_is_bounded_redacted_and_separate_from_action_authority() {
    let keys = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([4; 32]))).unwrap();
    let payload = payload();
    let cursor = HistoryCursor::seal(&payload, &keys).unwrap();
    assert!(cursor.as_str().len() < 4096);
    assert_eq!(
        cursor.open(&keys, 99).unwrap().payload.before_message_id,
        payload.before_message_id
    );
    assert!(matches!(cursor.open(&keys, 100), Err(OpenError::Expired)));
    assert!(cursor.open(&keys, 9).is_err());
    assert!(!format!("{cursor:?}").contains(cursor.as_str()));
    for kind in [
        TokenKind::EventRef,
        TokenKind::ThreadRef,
        TokenKind::MessageRef,
        TokenKind::ReplyTarget,
    ] {
        assert!(seal::open::<HistoryCursorPayload>(&keys, kind, cursor.as_str()).is_err());
        let raw = seal::seal(&keys, kind, &payload).unwrap();
        assert!(raw.parse::<HistoryCursor>().is_err());
        let deserialized: HistoryCursor = serde_json::from_value(json!(raw)).unwrap();
        assert!(deserialized.open(&keys, 20).is_err());
    }
    assert!(format!("cmaeg1.hc.{}", "x".repeat(4096))
        .parse::<HistoryCursor>()
        .is_err());
}
#[test]
fn cursor_checks_unsigned_boundary_scope_and_fixed_contract() {
    for case in 0..10 {
        let mut p = payload();
        match case {
            0 => p.before_message_id = p.anchor_message_id.clone(),
            1 => p.before_message_id = "18446744073709551616".into(),
            2 => p.before_message_id = "0".into(),
            3 => p.page_size = 26,
            4 => p.origin.channel_type = 3,
            5 => p.origin.parent_id = None,
            6 => p.origin.guild_id = None,
            7 => p.event_sha256 = "bad".into(),
            8 => p.issued_at = p.expires_at,
            _ => p.custody = "g".repeat(64),
        }
        assert!(!p.valid(), "case {case}");
    }
    for kind in [0, 1, 2, 5, 10, 11, 12, 13] {
        let origin = HistoryOrigin {
            channel_id: "99".into(),
            channel_type: kind,
            guild_id: (kind != 1).then(|| "100".into()),
            parent_id: matches!(kind, 10..=12).then(|| "98".into()),
        };
        assert!(origin.valid());
    }
}
#[test]
fn absent_or_false_history_consent_preserves_historical_source_serialization() {
    let legacy = json!({"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"}});
    let absent: Source = serde_json::from_value(legacy.clone()).unwrap();
    let mut explicit = legacy.clone();
    explicit["conversation_history"] = false.into();
    let false_source: Source = serde_json::from_value(explicit.clone()).unwrap();
    assert_eq!(absent, false_source);
    assert_eq!(serde_json::to_value(&absent).unwrap(), legacy);
    assert_eq!(
        serde_json::to_vec(&absent).unwrap(),
        serde_json::to_vec(&false_source).unwrap()
    );
    assert!(serde_json::to_value(&absent)
        .unwrap()
        .get("conversation_history")
        .is_none());
    explicit["conversation_history"] = true.into();
    let opted: Source = serde_json::from_value(explicit).unwrap();
    assert_ne!(
        serde_json::to_vec(&absent).unwrap(),
        serde_json::to_vec(&opted).unwrap()
    );
    assert!(!serde_json::to_value(&opted).unwrap()["thread_history"]
        .as_bool()
        .unwrap_or(false));
}
