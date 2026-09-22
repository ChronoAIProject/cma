//! Public channel schema deliberately excludes every retained secret value.
use cmaeg_core::{
    channel::{ChannelDefinition, PartitionRule, ProviderBinding, ReplyPolicy, Source},
    ids::RecordId,
    profile::{AgentProfileSnapshot, NyxIdIdentity},
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PublicChannelDefinition {
    pub schema_version: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub record_id: RecordId,
    pub owner: NyxIdIdentity,
    pub profile: AgentProfileSnapshot,
    pub profile_digest: String,
    pub provider: ProviderBinding,
    pub sources: Vec<Source>,
    pub partition: PartitionRule,
    pub reply: ReplyPolicy,
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
            owner: value.owner.clone(),
            profile: value.profile.clone(),
            profile_digest: value.profile_digest.clone(),
            provider: value.provider.clone(),
            sources: value.sources.clone(),
            partition: value.partition.clone(),
            reply: value.reply.clone(),
            secrets: PublicSecrets {
                agent_key_hash: value.secrets.agent_key_hash.clone(),
            },
            created_at: value.created_at.clone(),
            version,
        }
    }
}
