//! `cmaeg1.md` media links: sealing, every opening invariant, redaction and
//! the token limit. Tokens are sealed at runtime; none is committed.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::Activity,
    channel::{Platform, Secret},
    ids::ConversationId,
    seal::{
        event_ref::activity_digest,
        media_link::{open, seal, MediaLinkPayload, MediaLinkRef, MAX_MEDIA_LINK_TTL},
        ChannelPointer, EventRef, EventRefPayload, MasterKeyRing, OpenError, ReplyAddress,
        SealError, MAX_TOKEN_LENGTH,
    },
};

const NOW: i64 = 1_000;
const KEY_HASH: [u8; 32] = [0xab; 32];

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("3:{}", STANDARD.encode([3; 32]))).unwrap()
}

/// A digest-verified relay event expiring at `NOW + 1800`, with a live reply
/// token that minting must blank.
fn event(keys: &MasterKeyRing) -> EventRefPayload {
    let activity: Activity = serde_json::from_str(include_str!("fixtures/activity.json")).unwrap();
    let channel_id = ChannelPointer {
        v: 1,
        record_id: "record-1".parse().unwrap(),
        record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
    }
    .seal_id(keys)
    .unwrap();
    EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: Some(activity_digest(&activity)),
        conversation_id: ConversationId::derive(keys, &channel_id, 1, "chat-1"),
        channel_id,
        record_id: "record-1".parse().unwrap(),
        event_id: activity.event_id,
        partition_key: "chat-1".into(),
        expires_at: NOW + 1_800,
        reply: ReplyAddress::NyxidRelay {
            thread_reply: false,
            message_id: "message-1".into(),
            route_id: "route-1".into(),
            platform: Platform::Telegram,
            reply_token: Secret::new(format!("reply-{}", "r".repeat(24))),
            reply_token_expires_at: NOW + 600,
        },
        consumer: None,
        lineage: None,
    }
}

fn payload(keys: &MasterKeyRing, ttl: i64) -> MediaLinkPayload {
    MediaLinkPayload::new(
        "cma_codex".parse().unwrap(),
        "binding-1".parse().unwrap(),
        &KEY_HASH,
        event(keys),
        2,
        NOW,
        ttl,
    )
    .unwrap()
}

/// Seal `payload` with no invariant enforced, as a forger with the key could.
fn sealed(keys: &MasterKeyRing, payload: &MediaLinkPayload) -> MediaLinkRef {
    seal(keys, payload).unwrap()
}

fn format_error(result: Result<MediaLinkPayload, OpenError>) {
    assert!(
        matches!(result, Err(OpenError::Format)),
        "expected a format error, got {result:?}"
    );
}

#[test]
fn round_trip() {
    let keys = keys();
    let minted = payload(&keys, 120);
    assert_eq!((minted.issued_at, minted.expires_at), (NOW, NOW + 120));
    assert_eq!(minted.agent_key_hash, "ab".repeat(32));
    assert_eq!(minted.jti.len(), 32);
    // The link never carries reply authority.
    let ReplyAddress::NyxidRelay { reply_token, .. } = &minted.event.reply else {
        panic!("relay event expected");
    };
    assert_eq!(reply_token.as_str(), "");
    let token = sealed(&keys, &minted);
    assert!(token.expose_header_value().starts_with("cmaeg1.md.3."));
    assert!(token.expose_header_value().len() <= MAX_TOKEN_LENGTH);
    let parsed: MediaLinkRef = token.expose_header_value().parse().unwrap();
    assert_eq!(open(&keys, &parsed, NOW).unwrap(), minted);
    // Each mint draws a fresh jti.
    assert_ne!(payload(&keys, 120).jti, minted.jti);
}

#[test]
fn tampered_token_is_format() {
    let keys = keys();
    let token = sealed(&keys, &payload(&keys, 120));
    let mut raw = token.expose_header_value().to_owned();
    let last = raw.pop().unwrap();
    raw.push(if last == 'A' { 'B' } else { 'A' });
    format_error(open(&keys, &raw.parse().unwrap(), NOW));
    // Another key ring cannot open it either.
    let other = MasterKeyRing::parse(&format!("3:{}", STANDARD.encode([4; 32]))).unwrap();
    format_error(open(&other, &token, NOW));
}

#[test]
fn wrong_kind_is_rejected() {
    let keys = keys();
    let event_ref = EventRef::seal(&event(&keys), &keys).unwrap();
    assert!(matches!(
        event_ref.as_str().parse::<MediaLinkRef>(),
        Err(OpenError::Format)
    ));
    // An event_ref's ciphertext relabelled as a link fails authentication.
    let relabelled = event_ref.as_str().replacen("cmaeg1.ev.", "cmaeg1.md.", 1);
    format_error(open(&keys, &relabelled.parse().unwrap(), NOW));
}

#[test]
fn ttl_above_600_is_format() {
    let keys = keys();
    // The constructor caps the window itself.
    let capped = payload(&keys, 10_000);
    assert_eq!(capped.expires_at - capped.issued_at, MAX_MEDIA_LINK_TTL);
    assert!(open(&keys, &sealed(&keys, &capped), NOW).is_ok());
    let mut forged = capped;
    forged.expires_at = forged.issued_at + MAX_MEDIA_LINK_TTL + 1;
    format_error(open(&keys, &sealed(&keys, &forged), NOW));
}

#[test]
fn expired_is_expired() {
    let keys = keys();
    let token = sealed(&keys, &payload(&keys, 120));
    assert!(open(&keys, &token, NOW + 119).is_ok());
    assert!(matches!(
        open(&keys, &token, NOW + 120),
        Err(OpenError::Expired)
    ));
    // A link issued in the future is malformed, not early.
    format_error(open(&keys, &token, NOW - 1));
}

#[test]
fn expiry_past_event_is_format() {
    let keys = keys();
    let mut short = event(&keys);
    short.expires_at = NOW + 30;
    let minted = MediaLinkPayload::new(
        "cma_codex".parse().unwrap(),
        "binding-1".parse().unwrap(),
        &KEY_HASH,
        short,
        0,
        NOW,
        120,
    )
    .unwrap();
    // Minting ends the window at the event's own expiry.
    assert_eq!(minted.expires_at, NOW + 30);
    let mut forged = minted;
    forged.expires_at = NOW + 31;
    format_error(open(&keys, &sealed(&keys, &forged), NOW));
}

#[test]
fn bad_jti_is_format() {
    let keys = keys();
    let minted = payload(&keys, 120);
    let mut upper = minted.clone();
    upper.jti = upper
        .jti
        .to_uppercase()
        .replace(|c: char| c.is_ascii_digit(), "A");
    format_error(open(&keys, &sealed(&keys, &upper), NOW));
    let mut short = minted.clone();
    short.jti.pop();
    format_error(open(&keys, &sealed(&keys, &short), NOW));
    let mut hash = minted;
    hash.agent_key_hash.push('a');
    format_error(open(&keys, &sealed(&keys, &hash), NOW));
}

#[test]
fn missing_digest_is_format() {
    let keys = keys();
    let mut minted = payload(&keys, 120);
    minted.event.activity_sha256 = None;
    format_error(open(&keys, &sealed(&keys, &minted), NOW));
    let mut invalid = payload(&keys, 120);
    invalid.event.partition_key.clear();
    format_error(open(&keys, &sealed(&keys, &invalid), NOW));
    let mut index = payload(&keys, 120);
    index.index = 16;
    format_error(open(&keys, &sealed(&keys, &index), NOW));
    let mut version = payload(&keys, 120);
    version.v = 2;
    format_error(open(&keys, &sealed(&keys, &version), NOW));
}

#[test]
fn nonempty_reply_token_is_format() {
    let keys = keys();
    let mut minted = payload(&keys, 120);
    if let ReplyAddress::NyxidRelay { reply_token, .. } = &mut minted.event.reply {
        *reply_token = Secret::new("restored");
    }
    format_error(open(&keys, &sealed(&keys, &minted), NOW));
    // Other reply addresses carry no token to blank.
    let mut telegram = payload(&keys, 120);
    telegram.event.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "-100".into(),
        message_id: "5".into(),
        thread_id: None,
    };
    assert!(open(&keys, &sealed(&keys, &telegram), NOW).is_ok());
}

#[test]
fn debug_is_redacted() {
    let keys = keys();
    let minted = payload(&keys, 120);
    let token = sealed(&keys, &minted);
    let shown = format!("{minted:?} {token:?}");
    assert_eq!(format!("{token:?}"), "MediaLinkRef([redacted])");
    for secret in [
        minted.jti.as_str(),
        minted.agent_key_hash.as_str(),
        token.expose_header_value(),
        "chat-1",
    ] {
        assert!(!shown.contains(secret), "{secret} leaked into {shown}");
    }
}

#[test]
fn oversized_partition_key_returns_too_large() {
    let keys = keys();
    let mut large = event(&keys);
    large.partition_key = "p".repeat(3 * 1024);
    large.reply = ReplyAddress::None;
    let minted = MediaLinkPayload::new(
        "cma_codex".parse().unwrap(),
        "binding-1".parse().unwrap(),
        &KEY_HASH,
        large,
        0,
        NOW,
        120,
    )
    .unwrap();
    assert!(matches!(seal(&keys, &minted), Err(SealError::TooLarge)));
}

#[test]
fn from_str_rejects_long_and_unprefixed() {
    let keys = keys();
    let raw = sealed(&keys, &payload(&keys, 120))
        .expose_header_value()
        .to_owned();
    assert!(raw.parse::<MediaLinkRef>().is_ok());
    let long = format!("cmaeg1.md.{}", "a".repeat(MAX_TOKEN_LENGTH));
    for bad in [
        long.as_str(),
        "",
        "cmaeg1.md",
        "Cmaeg1.md.x",
        &raw["cmaeg1.".len()..],
    ] {
        assert!(
            matches!(bad.parse::<MediaLinkRef>(), Err(OpenError::Format)),
            "{bad:.40}"
        );
    }
}
