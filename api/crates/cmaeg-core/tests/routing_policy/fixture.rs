//! A routed channel every routing test edits.
#![allow(dead_code)] // Each test binary that includes this uses a subset.
use cmaeg_core::{channel::ChannelDefinition, profile::EventWorkerProfile, ValidationError};
use ring::digest::{digest, SHA256};
use serde_json::{json, Value};

pub const REVIEWER: &str = "csm_00000000000a";
pub const NYX: &str = "csm_00000000000b";
/// The Lark relay's only route; NyxID relay sends require a UUID.
pub const ROUTE: &str = "5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9f";
pub const OWNER: &str = "user-1";

/// A consumer's agent key, spelled with a low-entropy pattern so it can never
/// be mistaken for a real credential.
pub fn agent_key(consumer: &str) -> String {
    format!("nyx_fixture_{consumer}")
}

/// `sha256:` and the lowercase hex digest of `secret`.
pub fn hash(secret: &str) -> String {
    let bytes = digest(&SHA256, secret.as_bytes());
    let hex: String = bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

fn base() -> Value {
    serde_json::from_str(include_str!("../fixtures/channel.json")).unwrap()
}

/// The fixture profile, served by `provider`.
pub fn profile(provider: &str) -> Value {
    let mut profile = base()["profile"].clone();
    profile["metadata"]["provider"] = provider.into();
    profile
}

pub fn digest_of(profile: &Value) -> String {
    serde_json::from_value::<EventWorkerProfile>(profile.clone())
        .unwrap()
        .digest()
}

/// An additional consumer at its defaults, with a profile target.
pub fn consumer(id: &str, name: &str, provider: &str) -> Value {
    let profile = profile(provider);
    json!({"id": id, "name": name, "profile_digest": digest_of(&profile), "profile": profile,
        "provider": {"slug": provider, "binding_id": format!("binding-{name}"), "level": "l1"},
        "agent_key_hash": hash(&agent_key(id)), "created_at": "2026-10-01T00:00:00Z"})
}

pub fn telegram() -> Value {
    json!({"type": "telegram", "bot_id": "77", "bot_username": "golden_bot", "admission": {"type": "all"}})
}

pub fn relay(platform: &str, route: &str) -> Value {
    json!({"type": "nyxid_relay", "issuer": "https://nyx.example",
        "jwks_url": "https://nyx.example/.well-known/jwks.json", "key_id": "key-1",
        "route_ids": [route], "platform": platform, "bot_username": format!("{platform}_bot"),
        "admission": {"type": "all"}})
}

pub fn custom() -> Value {
    json!({"type": "custom", "auth": {"type": "hmac", "key_id": "key-1"}})
}

/// A routed channel on native Telegram, a Lark relay and a custom source.
/// `default` is presented as `lead`; `reviewer` (cma_codex) and `nyx`
/// (nyxbot) are additional consumers; every event goes to `default`.
pub fn routed() -> Value {
    routed_with_key(&agent_key("default"))
}

/// Use an explicit fixture key when exercising public credential parsing.
pub fn routed_with_key(default_key: &str) -> Value {
    let mut value = base();
    value["sources"] = json!([telegram(), relay("lark", ROUTE), custom()]);
    value["partition"]["key"] = "fixed".into();
    value["reply"]["sinks"] = json!([{"type": "source"},
        {"type": "result_webhook", "url": "https://receiver.example/events", "hmac_key_id": "key-1"}]);
    value["profile_digest"] = digest_of(&value["profile"]).into();
    value["secrets"] = json!({
        "agent_key_hash": hash(default_key),
        "provider_credential": default_key,
        "channel_key": "fixture-channel-key",
        "telegram_bot_token": "77:fixture",
        "telegram_webhook_secret": "fixture-webhook",
        "hmac_secrets": [["key-1", "Zml4dHVyZQ=="]],
        "consumer_keys": [[REVIEWER, agent_key(REVIEWER)], [NYX, agent_key(NYX)]]
    });
    value["routing"] = json!({
        "version": 1,
        "default_consumer": {"name": "lead"},
        "consumers": [consumer(REVIEWER, "reviewer", "cma_codex"), consumer(NYX, "nyx", "nyxbot")],
        "default_targets": ["default"]
    });
    value
}

pub fn definition(value: Value) -> ChannelDefinition {
    serde_json::from_value(value).unwrap()
}

/// The routed fixture with `edit` applied must be refused exactly so.
pub fn expect(edit: impl FnOnce(&mut Value), pointer: &str, message: &str) {
    let mut value = routed();
    edit(&mut value);
    let shown = value["routing"].to_string();
    assert_eq!(
        definition(value).validate(),
        Err(ValidationError::new(pointer, message)),
        "{shown}"
    );
}

/// The routed fixture with `edit` applied must be accepted.
pub fn accept(edit: impl FnOnce(&mut Value)) {
    let mut value = routed();
    edit(&mut value);
    definition(value).validate().unwrap();
}
