//! CMAEG HTTP DTOs deliberately independent of the gateway crates.
use layer2_core::bots::{BotAdmission, BotReply};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize)]
pub(super) struct Policy {
    pub schema_version: u16,
    pub name: String,
    pub profile: Value,
    pub provider: ProviderRequest,
    pub sources: Vec<SourceRequest>,
    pub partition: Value,
    pub reply: Value,
}
#[derive(Serialize)]
pub(super) struct ProviderRequest {
    pub slug: String,
}
#[derive(Serialize)]
pub(super) struct SourceRequest {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub issuer: String,
    pub key_id: String,
    pub route_ids: Vec<String>,
    pub platform: &'static str,
    pub admission: BotAdmission,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
}
#[derive(Serialize)]
pub(super) struct Credentials<'a> {
    pub agent_key: &'a str,
    pub channel_key: &'a str,
}
#[derive(Serialize)]
pub(super) struct Create<'a> {
    #[serde(flatten)]
    pub policy: Policy,
    pub record_id: &'a str,
    pub credentials: Credentials<'a>,
}
#[derive(Serialize)]
pub(super) struct Update<'a> {
    #[serde(flatten)]
    pub policy: Policy,
    pub expected_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials<'a>>,
}
// Responses intentionally accept additive fields. Unknown protocol levels or
// source variants are validated by the adapter and never interpreted as success.
#[derive(Deserialize)]
pub(super) struct Channel {
    pub channel_id: String,
    pub record_id: String,
    pub version: u64,
    pub provider: Provider,
    pub endpoints: Endpoints,
    pub definition: Definition,
}
#[derive(Deserialize)]
pub(super) struct Provider {
    pub slug: String,
    pub binding_id: String,
    pub level: String,
    pub refused: Vec<Refusal>,
}
#[derive(Deserialize)]
pub(super) struct Refusal {
    #[serde(rename = "field")]
    pub _field: String,
}

#[derive(Deserialize)]
pub(super) struct Endpoints {
    pub nyxid_callback_url: String,
    pub custom_event_url: String,
}
#[derive(Deserialize)]
pub(super) struct Definition {
    pub record_id: String,
    pub owner: Owner,
    pub profile: Value,
    pub sources: Vec<Source>,
    pub secrets: Secrets,
    pub creation: Option<Creation>,
}
#[derive(Deserialize)]
pub(super) struct Owner {
    pub subject: String,
}
#[derive(Deserialize)]
pub(super) struct Source {
    #[serde(rename = "type")]
    pub kind: String,
    pub issuer: String,
    pub key_id: String,
    pub route_ids: Vec<String>,
    pub platform: String,
    pub bot_username: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct Secrets {
    pub agent_key_hash: String,
}
#[derive(Deserialize)]
pub(super) struct Creation {
    pub ready: bool,
}
#[derive(Deserialize)]
pub(super) struct Test {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: String,
}
pub(super) fn reply(reply: &BotReply) -> Value {
    json!({"auto_final":reply.auto_final,"ack_after_ms":reply.ack_after_ms,"sinks":[{"type":"source"}]})
}
