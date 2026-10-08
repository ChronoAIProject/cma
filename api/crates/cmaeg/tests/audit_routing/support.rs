//! Channel shapes, sealing and fakes shared by the routing audit tests.
use super::{fake_cma::FakeCma, routing_fixture};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg::channels::definition::channel_pointer;
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, RecordId},
    profile::canonical_json,
    seal::{seal_blob, MasterKeyRing},
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Distinctive, so the hygiene test can prove it never reaches the output.
pub const OWNER: &str = "routing-audit-owner-fixture";

pub fn key_ring() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD_NO_PAD.encode([1_u8; 32]))).unwrap()
}

pub fn non_human() -> BTreeSet<String> {
    BTreeSet::from(["cma_codex".to_owned()])
}

pub fn records(cma: &FakeCma) -> cmaeg::records::RecordClient {
    cmaeg::records::RecordClient::new(&cma.base_url, "cma-service").unwrap()
}

/// The core's routed channel: Telegram, a Lark relay and a custom source;
/// `reviewer` (cma_codex) and `nyx` (nyxbot) beside `default`, every event
/// to `default`, and addressing on, so `reviewer` is mentionable.
pub fn routed() -> Value {
    routing_fixture::routed()
}

/// The routed channel with every directive off: `default` receives every
/// event while its provider answers non-human actors.
pub fn routed_to_default() -> Value {
    let mut value = routed();
    value["routing"]["addressing"] =
        json!({"mentions":false,"command":false,"event_field":false,"unknown_target_notice":false});
    value
}

/// The routed channel without routing: exactly what every image reads.
pub fn legacy() -> Value {
    let mut value = routed();
    value.as_object_mut().unwrap().remove("routing");
    value["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    value
}

/// Two relays on one channel, which only the extended source rules accept.
pub fn multi_relay() -> Value {
    let mut value = legacy();
    value["sources"] = json!([
        routing_fixture::relay("lark", routing_fixture::ROUTE),
        routing_fixture::relay("slack", "6c4f1d7f-2d2e-4e4a-8b9f-3a6c7d8e9f0a")
    ]);
    value["partition"] = json!({"version":1,"key":"conversation"});
    value["reply"]["sinks"] = json!([{"type":"source"}]);
    value
}

/// A chat-keyed channel whose `custom_key` is inert: no custom source.
pub fn inert_custom_key() -> Value {
    let mut value = legacy();
    value["sources"] = json!([routing_fixture::relay("lark", routing_fixture::ROUTE)]);
    value["partition"] = json!({"version":1,"key":"conversation","custom_key":"fixed"});
    value["reply"]["sinks"] = json!([{"type":"source"}]);
    value
}

/// A channel with every source detached.
pub fn zero_sources() -> Value {
    let mut value = legacy();
    value["sources"] = json!([]);
    value["reply"]["sinks"] = json!([
        {"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"key-1"}
    ]);
    value
}

pub fn seal(keys: &MasterKeyRing, definition: &ChannelDefinition) -> Vec<u8> {
    seal_blob(keys, canonical_json(definition.to_blob_json()).as_bytes()).unwrap()
}

/// Store `value` as a valid definition owned by [`OWNER`], optionally with
/// a reserved creation, and return it with its channel ID.
pub fn seed(
    cma: &FakeCma,
    keys: &MasterKeyRing,
    mut value: Value,
    created: bool,
) -> (ChannelDefinition, Option<ChannelId>) {
    let id = uuid::Uuid::new_v4().to_string();
    let channel = created.then(|| {
        channel_pointer(&cma.base_url, &RecordId::new(id.clone()).unwrap())
            .unwrap()
            .seal_id(keys)
            .unwrap()
    });
    value["record_id"] = id.clone().into();
    value["owner"]["subject"] = OWNER.into();
    value["profile"]["metadata"]["owner"]["subject"] = OWNER.into();
    if let Some(channel) = &channel {
        value["creation"] =
            json!({"channel_id":channel,"fingerprint":"sha256:fixture","ready":true});
    }
    let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.profile_digest = definition.profile.digest();
    definition.validate().unwrap();
    cma.seed(&id, OWNER, seal(keys, &definition));
    (definition, channel)
}

pub fn hash(channel: &Option<ChannelId>) -> String {
    cmaeg::telemetry::channel_hash(channel.as_ref().unwrap().as_str())
}
