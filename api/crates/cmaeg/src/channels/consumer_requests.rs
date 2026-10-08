//! Consumer management bodies. Missing patches retain the stored value.
use super::requests::ProviderRequest;
use cmaeg_core::{
    channel::{ConsumerReply, PublishPolicy, Secret},
    ids::ConsumerName,
    profile::EventWorkerProfile,
};
use serde::{Deserialize, Deserializer};
use utoipa::ToSchema;

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerCreateRequest {
    pub expected_version: u64,
    pub name: ConsumerName,
    #[serde(default)]
    pub label: Option<String>,
    pub provider: ProviderRequest,
    pub profile: EventWorkerProfile,
    pub credentials: ConsumerCredentials,
    #[serde(default = "yes")]
    pub mentionable: bool,
    #[serde(default)]
    pub non_human: Option<bool>,
    #[serde(default)]
    pub reply: ConsumerReply,
    #[serde(default)]
    pub publish: PublishPolicy,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerCredentials {
    pub agent_key: Secret,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerCredentialPatch {
    #[serde(default)]
    pub agent_key: Option<Secret>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsumerUpdateRequest {
    pub expected_version: u64,
    pub name: ConsumerName,
    /// Omitted retains the label; null clears it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>)]
    pub label: Option<Option<String>>,
    /// Additional consumers only. A supplied null is invalid.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<EventWorkerProfile>)]
    pub profile: Option<Option<EventWorkerProfile>>,
    /// Additional consumers only. A supplied null is invalid.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<ConsumerCredentialPatch>)]
    pub credentials: Option<Option<ConsumerCredentialPatch>>,
    #[serde(default)]
    pub mentionable: Option<bool>,
    /// Omitted retains the override; null restores the provider default.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<bool>)]
    pub non_human: Option<Option<bool>>,
    #[serde(default)]
    pub reply: Option<ConsumerReply>,
    #[serde(default)]
    pub publish: Option<PublishPolicy>,
}

fn yes() -> bool {
    true
}

// Serde normally merges missing and null Options. Presence is needed both
// for nullable patches and for rejecting forbidden default-consumer fields.
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::deserialize(deserializer).map(Some)
}
