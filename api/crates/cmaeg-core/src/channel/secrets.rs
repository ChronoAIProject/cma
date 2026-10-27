//! Credentials remain private to encrypted gateway records.
use serde::{Deserialize, Deserializer};
use std::fmt;
use utoipa::ToSchema;

use crate::ids::ConsumerId;

/// A secret that only appears in the encrypted channel blob.
#[derive(Clone, PartialEq, Eq, ToSchema)]
#[schema(value_type = String)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(String::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelSecrets {
    #[serde(default, skip_serializing)]
    pub native_lark: Option<super::NativeLarkCredentials>,
    pub agent_key_hash: String,
    #[serde(default, skip_serializing)]
    pub whatsapp: Option<super::WhatsappCredentials>,
    #[serde(default, skip_serializing)]
    pub channel_key: Option<Secret>,
    #[serde(default, skip_serializing)]
    pub hmac_secrets: Vec<(String, Secret)>,
    #[serde(default, skip_serializing)]
    pub provider_credential: Option<Secret>,
    #[serde(default, skip_serializing)]
    pub telegram_bot_token: Option<Secret>,
    #[serde(default, skip_serializing)]
    pub discord_bot_token: Option<Secret>,
    #[serde(default, skip_serializing)]
    pub telegram_webhook_secret: Option<Secret>,
    /// Additional consumers' agent keys, which are their provider
    /// credentials; only ever in the sealed blob, and only when non-empty.
    #[serde(default, skip_serializing)]
    pub consumer_keys: Vec<(ConsumerId, Secret)>,
}
