//! I3: the references and interaction cards issued for the default consumer,
//! pinned byte for byte.
//!
//! An `event_ref`, `message_ref` or `reply_target_ref` is sealed over the
//! canonical JSON of its payload, and an interaction card is stored as the
//! JSON of its record. Outstanding tokens and stored cards must keep opening
//! on every image of a rollout, so a payload or card that carries no consumer
//! must serialize exactly as it did before consumers existed: no `consumer`
//! key, no `lineage` key, and cards sealed under kind `in`.
//!
//! The fixtures were generated on the parent commit of the change that added
//! consumer-bound references. Updating one is a reviewed contract change,
//! never a way to make a test pass (see `fixture.rs`).
use super::{fake_cma::FakeCma, fixture::assert_golden};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    interactions::{Card, Destination, InteractionStore, PreparedCard},
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use cmaeg_core::{
    channel::{Platform, Secret},
    ids::{BindingId, ChannelId, ConversationId, EventId, ProviderSlug, RecordId},
    profile::canonical_json,
    seal::{
        open, open_interaction, seal, seal_interaction, EventRefPayload, MasterKeyRing,
        MessageRefPayload, ReplyAddress, ReplyTargetPayload, TokenKind,
    },
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

/// The keys a newer image adds to references and cards; a default-consumer
/// payload must carry neither, at any depth.
const CONSUMER_KEYS: [&str; 2] = ["consumer", "lineage"];

/// A low-entropy key ring, fixed so the identities it seals stay reproducible.
fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap()
}

fn assert_no_consumer_keys(name: &str, value: &Value) {
    match value {
        Value::Object(object) => {
            for key in CONSUMER_KEYS {
                assert!(!object.contains_key(key), "{name}: carries `{key}`");
            }
            for nested in object.values() {
                assert_no_consumer_keys(name, nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                assert_no_consumer_keys(name, nested);
            }
        }
        _ => {}
    }
}

/// Pin the canonical plaintext of `payload`, and prove it is exactly what a
/// token of `kind` seals: opening the token yields the same canonical JSON.
fn assert_sealed_plaintext<T: Serialize>(name: &str, kind: TokenKind, payload: &T) {
    let plaintext = canonical_json(serde_json::to_value(payload).unwrap());
    assert_golden(&format!("references/{name}.json"), &plaintext);
    let value: Value = serde_json::from_str(&plaintext).unwrap();
    assert_no_consumer_keys(name, &value);
    let ring = keys();
    let token = seal(&ring, kind, payload).unwrap();
    let opened = open::<Value>(&ring, kind, &token).unwrap();
    assert_eq!(canonical_json(opened.payload), plaintext, "{name}");
}

fn event(reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: Some("a".repeat(64)),
        channel_id: ChannelId::new("cmaeg1.ch.golden-references").unwrap(),
        record_id: RecordId::new("8a6c1f9e-2b7d-4c3e-9f10-5d4b3a291807").unwrap(),
        event_id: EventId::new("message-1").unwrap(),
        partition_key: "42\u{1f}7".into(),
        conversation_id: ConversationId::new(format!("conv_{}", "0".repeat(32))).unwrap(),
        expires_at: 4_102_444_800,
        reply,
        consumer: None,
        lineage: None,
    }
}

fn reply_addresses() -> [(&'static str, ReplyAddress); 5] {
    [
        (
            "event_ref_telegram",
            ReplyAddress::Telegram {
                bot_id: "123456".into(),
                chat_id: "-100123".into(),
                message_id: "1001".into(),
                thread_id: Some("7".into()),
            },
        ),
        (
            "event_ref_nyxid_relay",
            ReplyAddress::NyxidRelay {
                thread_reply: false,
                message_id: "message-1".into(),
                route_id: "route-1".into(),
                platform: Platform::Lark,
                reply_token: Secret::new("relay-reply-fixture"),
                reply_token_expires_at: 4_102_444_800,
            },
        ),
        (
            "event_ref_result_webhook",
            ReplyAddress::ResultWebhook {
                url: "https://results.example/hook".into(),
                hmac_key_id: "key-1".into(),
            },
        ),
        (
            "event_ref_sync",
            ReplyAddress::Sync {
                waiter: "waiter-1".into(),
            },
        ),
        ("event_ref_none", ReplyAddress::None),
    ]
}

#[test]
fn default_event_refs_keep_their_canonical_plaintext_for_every_reply_address() {
    for (name, reply) in reply_addresses() {
        let payload = event(reply);
        payload.validate().unwrap();
        assert_sealed_plaintext(name, TokenKind::EventRef, &payload);
    }
}

#[test]
fn default_message_refs_keep_their_canonical_plaintext() {
    let plain = MessageRefPayload {
        discord_message: None,
        received_reaction: None,
        native_lark_effect: None,
        whatsapp_reaction: None,
        lark_effect: None,
        telegram_effect: None,
        channel_id: ChannelId::new("cmaeg1.ch.golden-references").unwrap(),
        event_id: EventId::new("message-1").unwrap(),
        platform_message_id: "42".into(),
        route_id: "route-1".into(),
        interaction_record_id: None,
        relay_message: None,
        consumer: None,
    };
    let interaction = MessageRefPayload {
        discord_message: None,
        received_reaction: None,
        native_lark_effect: None,
        whatsapp_reaction: None,
        lark_effect: None,
        telegram_effect: None,
        interaction_record_id: Some("6f1e2d3c-4b5a-4978-8a69-5b4c3d2e1f00".into()),
        ..plain.clone()
    };
    for (name, payload) in [
        ("message_ref", plain),
        ("message_ref_interaction", interaction),
    ] {
        payload.validate().unwrap();
        assert_sealed_plaintext(name, TokenKind::MessageRef, &payload);
    }
}

#[test]
fn default_reply_targets_keep_their_canonical_plaintext() {
    let payload = ReplyTargetPayload {
        v: 1,
        provider: ProviderSlug::new("nyxbot").unwrap(),
        binding_id: BindingId::new("binding-1").unwrap(),
        agent_key_hash: [9; 32],
        issued_at: 4_102_444_000,
        expires_at: 4_102_444_700,
        event: event(reply_addresses()[1].1.clone()),
    };
    assert_sealed_plaintext("reply_target", TokenKind::ReplyTarget, &payload);
}

const CARD_ID: &str = "0b9d8e7f-6a5b-4c3d-8e2f-1a0b9c8d7e6f";

/// A bound, clicked card, as `InteractionStore` writes it after delivery.
fn card() -> Card {
    serde_json::from_value(json!({
        "id": CARD_ID,
        "owner": "nyx-user-1",
        "channel_id": "cmaeg1.ch.golden-references",
        "event_id": "message-1",
        "route_id": "route-1",
        "actor": "7",
        "chat": "42",
        "thread": null,
        "expires_at": 4_102_444_800_i64,
        "revision": 1,
        "message": {"text": "Choose owner", "blocks": [{"type": "choice", "id": "owner", "prompt": "Personal or org?", "multi": false, "options": [{"id": "personal", "label": "Personal"}, {"id": "org", "label": "Organization"}]}]},
        "destination": {"outbound_id": "outbound-1", "platform_id": "42"},
        "reference": "cmaeg1.mr.golden-reference",
        "consumed": {"event_id": "click-1", "callback_id": "query-1", "action": 1, "occurred_at": "2026-09-22T00:00:00Z"},
        "superseded": false
    }))
    .unwrap()
}

#[test]
fn default_cards_keep_their_json() {
    let stored = serde_json::to_string(&card()).unwrap();
    assert_golden("references/card.json", &stored);
    assert_no_consumer_keys("card", &serde_json::from_str(&stored).unwrap());
}

/// The write path seals a default card under kind `in`, with the exact bytes
/// pinned above, and the read path opens it.
#[tokio::test]
async fn default_cards_are_stored_under_the_interaction_kind_with_identical_bytes() {
    let ring = Arc::new(keys());
    let cma = FakeCma::start(false).await;
    let stored = serde_json::to_vec(&card()).unwrap();
    cma.seed(
        CARD_ID,
        "nyx-user-1",
        seal_interaction(&ring, CARD_ID, &stored).unwrap(),
    );
    let providers = ProviderClient::new(
        ProviderRegistry::default(),
        Duration::from_secs(1),
        Duration::from_secs(1),
        1,
    )
    .unwrap();
    let store = InteractionStore::new(
        RecordClient::new(&cma.base_url, "cma-service").unwrap(),
        ring.clone(),
        Arc::new(providers),
    );
    let (loaded, version) = store.load(CARD_ID).await.unwrap();
    let mut prepared = PreparedCard {
        card: loaded,
        version,
        markup: Value::Null,
    };
    store
        .bind(
            &mut prepared,
            Destination {
                outbound_id: "outbound-1".into(),
                platform_id: "42".into(),
            },
        )
        .await
        .unwrap();
    let record = cma.records().remove(CARD_ID).unwrap();
    assert_eq!(record.version, 2, "the card was written back");
    assert!(record.blob.starts_with(b"cmaeg1.in."));
    let written = open_interaction(&ring, CARD_ID, &record.blob).unwrap();
    assert_eq!(written.payload, stored);
    assert_no_consumer_keys("card", &serde_json::from_slice(&written.payload).unwrap());
    cma.close().await;
}
