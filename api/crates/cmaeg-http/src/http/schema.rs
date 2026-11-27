//! Public channel schema deliberately excludes every retained secret value.
use cmaeg_core::{
    channel::{
        ChannelDefinition, PartitionRule, ProviderBinding, ReplyPolicy, RoutingPolicy, Source,
    },
    ids::RecordId,
    profile::{EventWorkerProfile, NyxIdIdentity},
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PublicChannelDefinition {
    pub schema_version: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub record_id: RecordId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation: Option<cmaeg_core::channel::ChannelCreation>,
    pub owner: NyxIdIdentity,
    pub profile: EventWorkerProfile,
    pub profile_digest: String,
    pub provider: ProviderBinding,
    pub sources: Vec<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    pub partition: PartitionRule,
    pub reply: ReplyPolicy,
    /// Multi-consumer routing, read-only here; absent on every channel
    /// without it. It carries consumers' public key hashes, never a key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingPolicy>,
    /// Immutable owner opt-in for this channel's authenticated Custom events.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub durable_custom_execution: bool,
    pub secrets: PublicSecrets,
    pub created_at: String,
    pub version: u64,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PublicSecrets {
    pub agent_key_hash: String,
}
impl PublicChannelDefinition {
    pub fn new(value: &ChannelDefinition, version: u64) -> Self {
        Self {
            schema_version: value.schema_version,
            name: value.name.clone(),
            record_id: value.record_id.clone(),
            creation: value.creation.clone(),
            owner: value.owner.clone(),
            profile: value.profile.clone(),
            profile_digest: value.profile_digest.clone(),
            provider: value.provider.clone(),
            sources: value.sources.clone(),
            connection_ref: value.connection_ref.clone(),
            partition: value.partition.clone(),
            reply: value.reply.clone(),
            routing: value.routing.clone(),
            durable_custom_execution: value.durable_custom_execution,
            secrets: PublicSecrets {
                agent_key_hash: value.secrets.agent_key_hash.clone(),
            },
            created_at: value.created_at.clone(),
            version,
        }
    }
}
