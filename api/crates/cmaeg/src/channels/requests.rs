//! Management request shapes, separate from construction and validation.
use cmaeg_core::{
    channel::{AdmissionPolicy, CustomAuth, ReplyPolicy, Secret},
    ids::{ProviderSlug, RecordId},
    profile::EventWorkerProfile,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// Request wrapper used by channel creation.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelCreateRequest {
    /// Optional UUID reserved by a durable creator before issuing the request.
    #[serde(default)]
    pub record_id: Option<RecordId>,
    pub schema_version: u16,
    pub name: String,
    pub profile: EventWorkerProfile,
    pub provider: ProviderRequest,
    pub sources: Vec<SourceRequest>,
    pub partition: cmaeg_core::channel::PartitionRule,
    pub reply: ReplyPolicy,
    pub credentials: CredentialsRequest,
}

/// Update body; credentials are retained when this field is absent.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelUpdateRequest {
    pub schema_version: u16,
    pub name: String,
    pub profile: EventWorkerProfile,
    pub provider: ProviderRequest,
    pub sources: Vec<SourceRequest>,
    pub partition: cmaeg_core::channel::PartitionRule,
    pub reply: ReplyPolicy,
    #[serde(default)]
    pub credentials: Option<CredentialPatch>,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequest {
    pub slug: ProviderSlug,
    #[serde(
        default,
        skip_serializing_if = "cmaeg_core::channel::ProviderTarget::is_profile"
    )]
    pub target: cmaeg_core::channel::ProviderTarget,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum SourceRequest {
    NyxidRelay {
        issuer: String,
        key_id: String,
        route_ids: Vec<String>,
        platform: cmaeg_core::channel::Platform,
        #[serde(default)]
        bot_username: Option<String>,
        admission: AdmissionPolicy,
    },
    Custom {
        auth: CustomAuth,
        #[serde(default)]
        prompt_template: Option<String>,
        #[serde(default)]
        schema: Option<Value>,
    },
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialsRequest {
    pub agent_key: Secret,
    #[serde(default)]
    pub channel_key: Option<Secret>,
    #[serde(default)]
    pub hmac_secrets: Vec<HmacSecretRequest>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialPatch {
    #[serde(default)]
    pub agent_key: Option<Secret>,
    #[serde(default)]
    pub channel_key: Option<Secret>,
    #[serde(default)]
    pub hmac_secrets: Option<Vec<HmacSecretRequest>>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HmacSecretRequest {
    pub key_id: String,
    pub secret: Secret,
}
