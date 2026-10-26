//! A consumer-bound hand-off event_ref still fits the 4,096-character token
//! bound with the largest values a reference carries.
//!
//! An event_ref travels in URLs and tool arguments, so a reference that grew
//! past `MAX_TOKEN_LENGTH` would make ingress refuse the event. These fixtures
//! fill a relay and a custom reference with the largest values they carry —
//! a 512-byte partition key (the most an event_ref seals verbatim), a
//! production-shaped channel id, the longest relay reply token shape the
//! fixtures sign, sized for a 4096-bit RSA signature, and, for custom events,
//! the largest event id — and then add a consumer and a four-hop lineage of
//! `csm_` publishers.
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use cmaeg_core::{
    activity::MAX_HOPS,
    channel::{Platform, ReplyChoice, Secret},
    ids::{ChannelId, ConsumerId, EventId, RecordId, MAX_ID_BYTES, MAX_SEALED_PARTITION_KEY_BYTES},
    seal::{
        ChannelPointer, EventRef, EventRefPayload, Lineage, MasterKeyRing, ReplyAddress, RootId,
        MAX_TOKEN_LENGTH,
    },
};
use serde_json::json;

const RECORD_ID: &str = "8a6c1f9e-2b7d-4c3e-9f10-5d4b3a291807";
/// What one consumer id and a four-hop lineage may add to a reference.
const MAX_CONSUMER_COST: usize = 320;

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("200:{}", STANDARD.encode([8; 32]))).unwrap()
}

/// A channel id sealed over an in-cluster record URL, longer than any the
/// fixtures use.
fn channel_id(keys: &MasterKeyRing) -> ChannelId {
    ChannelPointer {
        v: 1,
        record_url: format!(
            "http://example-host-01.example-namespace.svc.cluster.local:8080/api/v1/gateway/channel-records/{RECORD_ID}"
        ),
        record_id: RecordId::new(RECORD_ID).unwrap(),
    }
    .seal_id(keys)
    .unwrap()
}

/// A NyxID relay reply token with the claims the relay fixtures sign and a
/// 512-byte (4096-bit RSA) signature.
fn reply_token() -> String {
    let header = json!({"alg":"RS256","typ":"JWT","kid":"nyxid-relay-signing-key-2026-10"});
    let claims = json!({
        "iss": "https://nyx.example",
        "aud": "channel-relay/reply",
        "token_type": "relay_reply",
        "iat": 1_790_000_000_i64,
        "exp": 1_790_001_800_i64,
        "jti": "5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9f",
        "sub": "route:5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9f",
        "message_id": "wamid.HBgLMTU1NTAwMDExMTEVAgASGCA2RkQ3MEY4QjY3RkE0RDY2"
    });
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string()),
        URL_SAFE_NO_PAD.encode([0x5a; 512])
    )
}

/// Four hops, each published by a distinct generated consumer.
fn lineage() -> Lineage {
    Lineage {
        root: RootId::derive(
            &RecordId::new(RECORD_ID).unwrap(),
            &EventId::new("root-event").unwrap(),
        )
        .as_str()
        .to_owned(),
        hop: MAX_HOPS,
        path: (0..MAX_HOPS)
            .map(|index| ConsumerId::parse(&format!("csm_ffffffffff{index:02x}")).unwrap())
            .collect(),
        reply: ReplyChoice::Default,
    }
}

fn base(keys: &MasterKeyRing, event_id: String, reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: Some("f".repeat(64)),
        channel_id: channel_id(keys),
        record_id: RecordId::new(RECORD_ID).unwrap(),
        event_id: EventId::new(event_id).unwrap(),
        partition_key: "p".repeat(MAX_SEALED_PARTITION_KEY_BYTES),
        conversation_id: format!("conv_{}", "f".repeat(32)).parse().unwrap(),
        expires_at: 1_790_001_800,
        reply,
        consumer: None,
        lineage: None,
    }
}

fn relay(keys: &MasterKeyRing) -> EventRefPayload {
    base(
        keys,
        "wamid.HBgLMTU1NTAwMDExMTEVAgASGCA2RkQ3MEY4QjY3RkE0RDY2".into(),
        ReplyAddress::NyxidRelay {
            thread_reply: false,
            message_id: "wamid.HBgLMTU1NTAwMDExMTEVAgASGCA2RkQ3MEY4QjY3RkE0RDY2".into(),
            route_id: "5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9f".into(),
            platform: Platform::Whatsapp,
            reply_token: Secret::new(reply_token()),
            reply_token_expires_at: 1_790_001_800,
        },
    )
}

fn custom(keys: &MasterKeyRing) -> EventRefPayload {
    base(
        keys,
        "e".repeat(MAX_ID_BYTES),
        ReplyAddress::ResultWebhook {
            url: format!("https://results.example/{}", "w".repeat(488)),
            hmac_key_id: "k".repeat(64),
        },
    )
}

/// The sealed length of `payload` without and with a consumer and lineage.
fn lengths(payload: EventRefPayload, keys: &MasterKeyRing) -> (usize, usize) {
    payload.validate().unwrap();
    let legacy = EventRef::seal(&payload, keys).unwrap().as_str().len();
    let bound = EventRefPayload {
        consumer: Some(ConsumerId::parse("csm_ffffffffffff").unwrap()),
        lineage: Some(lineage()),
        ..payload
    };
    bound.validate().unwrap();
    let reference = EventRef::seal(&bound, keys).unwrap();
    assert_eq!(reference.open(keys, 0).unwrap().payload, bound);
    (legacy, reference.as_str().len())
}

#[test]
fn the_largest_consumer_bound_references_fit_the_token_bound() {
    let keys = keys();
    for (name, payload) in [("relay", relay(&keys)), ("custom", custom(&keys))] {
        let (legacy, bound) = lengths(payload, &keys);
        println!("{name}: {legacy} characters without a consumer, {bound} with one");
        assert!(
            bound <= MAX_TOKEN_LENGTH,
            "{name} reference is {bound} characters"
        );
        assert!(
            bound - legacy <= MAX_CONSUMER_COST,
            "{name}: a consumer and lineage cost {} characters",
            bound - legacy
        );
    }
}
