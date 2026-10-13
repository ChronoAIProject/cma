//! I1: a legacy channel definition keeps its stored bytes. For every legacy
//! shape the sealed plaintext is byte-identical to the blob recorded before
//! the multi-consumer changes, carries neither `routing` nor
//! `consumer_keys`, an older image can still parse it, and re-sealing what
//! this image reads back reproduces the same plaintext. A routed blob
//! round-trips with its consumer keys, which its public view never shows.
//!
//! Each fixture is built here rather than through management so the blob
//! depends only on the definition types; the management-written records are
//! pinned by the provider-capture goldens.
use super::{fixture::assert_golden, previous_schema};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    channel::ChannelDefinition,
    profile::canonical_json,
    seal::{open_blob, seal_blob, MasterKeyRing},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ISSUER: &str = "https://nyx.example";

/// A custom channel every fixture edits. Credentials are low-entropy
/// placeholders so the recorded blobs can never be mistaken for real ones.
fn base() -> Value {
    json!({
        "schema_version": 1,
        "name": "golden-channel",
        "record_id": "8a5e3c1e-5d1c-4f5e-9a3b-2c1d0e9f8a7b",
        "owner": {"kind": "human", "subject": "golden-owner"},
        "profile": {
            "schema_version": 1,
            "metadata": {
                "name": "Golden", "description": "A pinned profile",
                "owner": {"kind": "human", "subject": "golden-owner"},
                "provider": "cma_codex"
            },
            "model": {"access": "managed", "service": "openai", "api_type": "responses",
                "model": "gpt-test", "thinking_effort": "medium"},
            "permission": {"action_mode": "guarded", "service_scope": ["github"]},
            "context": {"skills": [], "system_prompt": "", "mcp_servers": []}
        },
        "profile_digest": "",
        "provider": {"slug": "cma_codex", "binding_id": "bnd_golden", "level": "l1"},
        "sources": [custom_hmac()],
        "partition": {"version": 1, "key": "per_event"},
        "reply": {"auto_final": true, "ack_after_ms": 3000, "sinks": [{"type": "source"}]},
        "secrets": {
            "agent_key_hash": format!("sha256:{}", "a".repeat(64)),
            "provider_credential": format!("nyx_{}", "golden".repeat(4)),
            "hmac_secrets": [["golden-hmac", STANDARD.encode("golden")]]
        },
        "created_at": "2026-10-01T00:00:00Z",
        "version": 3
    })
}

fn custom_hmac() -> Value {
    json!({"type": "custom", "auth": {"type": "hmac", "key_id": "golden-hmac"}})
}

fn custom_nyxid_key() -> Value {
    json!({"type": "custom", "auth": {"type": "nyxid_key", "key_hashes": [format!("sha256:{}", "b".repeat(64))]},
        "prompt_template": "Event {{type}}", "attachments": {"origins": ["https://files.example"]}})
}

fn telegram() -> Value {
    json!({"type": "telegram", "bot_id": "77", "bot_username": "golden_bot", "admission": {"type": "all"}})
}

fn relay(platform: &str) -> Value {
    let mut source = json!({"type": "nyxid_relay", "issuer": ISSUER,
        "jwks_url": format!("{ISSUER}/.well-known/jwks.json"), "key_id": "golden-key",
        "route_ids": ["route-1"], "platform": platform,
        "admission": {"type": "allowlist", "sender_ids": ["sender-1"]}});
    match platform {
        "telegram" => source["bot_username"] = "golden_bot".into(),
        "lark" | "feishu" => source["bot_id"] = "ou_golden".into(),
        _ => {
            source["bot_id"] = "golden-bot".into();
            source["trust_normalized"] = true.into();
        }
    }
    source
}

/// Apply `edit` to the base channel.
fn channel(edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = base();
    edit(&mut value);
    value
}

fn chat(sources: Value, key: &str) -> impl FnOnce(&mut Value) {
    let key = key.to_owned();
    move |value: &mut Value| {
        value["sources"] = sources;
        value["partition"]["key"] = key.into();
        let native = value["sources"]
            .as_array()
            .is_some_and(|list| list.iter().any(|source| source["type"] == "telegram"));
        value["secrets"]["channel_key"] = "golden-channel-key".into();
        if native {
            value["secrets"]["telegram_bot_token"] = "77:golden".into();
            value["secrets"]["telegram_webhook_secret"] = "golden-webhook".into();
        }
    }
}

/// Every legacy shape the blob must keep, by fixture name.
fn fixtures() -> Vec<(String, Value)> {
    let mut fixtures = vec![
        (
            "telegram".to_owned(),
            channel(chat(json!([telegram()]), "conversation_and_sender")),
        ),
        ("custom_hmac".into(), channel(|_| {})),
        (
            "custom_nyxid_key".into(),
            channel(|value| {
                value["sources"] = json!([custom_nyxid_key()]);
                value["partition"]["key"] = "event_subject".into();
                value["secrets"]["hmac_secrets"] = json!([]);
            }),
        ),
        (
            "telegram_relay_custom_fixed".into(),
            channel(chat(
                json!([telegram(), relay("lark"), custom_hmac()]),
                "fixed",
            )),
        ),
        (
            "sinks".into(),
            channel(|value| {
                chat(json!([relay("telegram")]), "thread")(value);
                value["reply"] = json!({"auto_final": false, "ack_after_ms": 0, "sinks": [
                    {"type": "source"}, {"type": "sync"},
                    {"type": "result_webhook", "url": "https://receiver.example/events", "hmac_key_id": "golden-hmac"}]});
            }),
        ),
        (
            "creation".into(),
            channel(|value| {
                value["creation"] = json!({"channel_id": "cmaeg1.ch.golden-creation",
                    "fingerprint": format!("sha256:{}", "c".repeat(64)), "ready": true});
            }),
        ),
        (
            "connection_ref".into(),
            channel(|value| {
                chat(json!([relay("telegram")]), "conversation")(value);
                value["connection_ref"] = "conn_golden".into();
                value["provider"]["target"] =
                    json!({"type": "existing_agent", "agent_id": "agt_golden"});
            }),
        ),
    ];
    for platform in ["telegram", "lark", "feishu", "discord", "slack", "whatsapp"] {
        fixtures.push((
            format!("relay_{platform}"),
            channel(chat(json!([relay(platform)]), "conversation_and_sender")),
        ));
    }
    fixtures
}

fn definition(name: &str, value: Value) -> ChannelDefinition {
    let mut definition: ChannelDefinition = serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("{name}: the fixture is not a definition: {error}"));
    definition.profile_digest = definition.profile.digest();
    definition
        .validate_legacy()
        .unwrap_or_else(|error| panic!("{name}: the fixture is not a legacy definition: {error}"));
    definition
}

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([5; 32]))).unwrap()
}

#[test]
fn legacy_blobs_keep_their_bytes_and_parse_with_the_previous_schema() {
    let keys = keys();
    for (name, value) in fixtures() {
        let definition = definition(&name, value);
        let blob = definition.to_blob_json();
        // Neither multi-consumer shape appears on a legacy channel.
        assert!(blob.get("routing").is_none(), "{name}");
        assert!(blob["secrets"].get("consumer_keys").is_none(), "{name}");
        let plaintext = canonical_json(blob);
        assert_golden(&format!("blobs/{name}.json"), &plaintext);

        // An older image reads, and would write back, exactly these bytes.
        let previous: previous_schema::ChannelDefinition = serde_json::from_str(&plaintext)
            .unwrap_or_else(|error| panic!("{name}: the previous schema refuses it: {error}"));
        assert_eq!(
            canonical_json(serde_json::to_value(&previous).unwrap()),
            plaintext,
            "{name}: the previous schema does not round-trip it"
        );

        // Sealing is lossless, and what this image reads back re-seals identically.
        let sealed = seal_blob(&keys, plaintext.as_bytes()).unwrap();
        let opened = open_blob(&keys, &sealed).unwrap().payload;
        assert_eq!(opened, plaintext.as_bytes(), "{name}");
        let restored: ChannelDefinition = serde_json::from_slice(&opened).unwrap();
        assert_eq!(restored, definition, "{name}");
        assert_eq!(canonical_json(restored.to_blob_json()), plaintext, "{name}");
    }
}

#[test]
fn the_previous_schema_cannot_read_a_custom_key() {
    // Why custom_key writes stay behind the dynamic-sources flag: an older
    // image refuses the whole definition, not just the new field.
    let mut definition = definition(
        "relay_telegram",
        channel(chat(json!([relay("telegram")]), "conversation")),
    );
    definition.partition.custom_key = Some(cmaeg_core::channel::PartitionKey::EventSubject);
    definition.validate().unwrap();
    let plaintext = canonical_json(definition.to_blob_json());
    let error = serde_json::from_str::<previous_schema::ChannelDefinition>(&plaintext).unwrap_err();
    assert!(error.to_string().contains("custom_key"), "{error}");
}

const CONSUMER: &str = "csm_0000000000a1";

/// The low-entropy agent key of the routed fixture's additional consumer.
fn consumer_key() -> String {
    format!("nyx_{}", "consumer".repeat(3))
}

/// The custom channel with a routing policy and one additional consumer, as
/// a writer behind the multi-consumer flag would store it.
fn routed() -> ChannelDefinition {
    let mut value = base();
    let mut consumer_profile = value["profile"].clone();
    consumer_profile["metadata"]["name"] = "Reviewer".into();
    let hash = format!("sha256:{:x}", Sha256::digest(consumer_key().as_bytes()));
    value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
        "consumers": [{"id": CONSUMER, "name": "reviewer", "profile": consumer_profile,
            "profile_digest": "", "provider": {"slug": "cma_codex", "binding_id": "bnd_reviewer", "level": "l1"},
            "agent_key_hash": hash, "created_at": "2026-10-01T00:00:00Z"}],
        "default_targets": ["default", CONSUMER]});
    value["secrets"]["consumer_keys"] = json!([[CONSUMER, consumer_key()]]);
    let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.profile_digest = definition.profile.digest();
    let consumer = &mut definition.routing.as_mut().unwrap().consumers[0];
    consumer.profile_digest = consumer.profile.digest();
    definition.validate().unwrap();
    definition
}

#[test]
fn a_routed_blob_round_trips_with_its_consumer_keys() {
    let keys = keys();
    let routed = routed();
    let plaintext = canonical_json(routed.to_blob_json());
    let sealed = seal_blob(&keys, plaintext.as_bytes()).unwrap();
    let opened = open_blob(&keys, &sealed).unwrap().payload;
    let restored: ChannelDefinition = serde_json::from_slice(&opened).unwrap();
    assert_eq!(restored, routed);
    assert_eq!(restored.secrets.consumer_keys, routed.secrets.consumer_keys);
    assert_eq!(
        restored.secrets.consumer_keys[0].1.as_str(),
        consumer_key().as_str()
    );
    assert_eq!(canonical_json(restored.to_blob_json()), plaintext);
}

#[test]
fn the_public_view_of_a_routed_channel_holds_no_key() {
    let routed = routed();
    let public = routed.to_public_json().to_string();
    assert!(public.contains("\"routing\""));
    for key in [consumer_key(), format!("nyx_{}", "golden".repeat(4))] {
        assert!(!public.contains(&key), "{public}");
    }
    assert!(!public.contains("consumer_keys"));
}

#[test]
fn the_previous_schema_cannot_read_routing_or_consumer_keys() {
    // Why routing writes stay behind the multi-consumer flag: an older image
    // refuses the whole definition, whichever of the two it carries.
    let routed = routed();
    let plaintext = canonical_json(routed.to_blob_json());
    let error = serde_json::from_str::<previous_schema::ChannelDefinition>(&plaintext).unwrap_err();
    assert!(error.to_string().contains("routing"), "{error}");
    let mut keys_only = routed.clone();
    keys_only.routing = None;
    let plaintext = canonical_json(keys_only.to_blob_json());
    let error = serde_json::from_str::<previous_schema::ChannelDefinition>(&plaintext).unwrap_err();
    assert!(error.to_string().contains("consumer_keys"), "{error}");
}
