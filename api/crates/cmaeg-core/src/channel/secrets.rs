//! Credentials remain private to encrypted gateway records.
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::fmt;
use utoipa::ToSchema;

use crate::ids::ConsumerId;

use super::ChannelDefinition;

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
    #[schema(ignore)]
    pub profile_revisions: Option<super::ProfileRevisions>,
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

impl ChannelDefinition {
    /// Serialize the private blob consumed by the sealing layer.
    pub fn to_blob_json(&self) -> Value {
        let mut object = serde_json::to_value(self)
            .expect("channel definition is serializable without private fields")
            .as_object()
            .cloned()
            .expect("channel definition is an object");
        let mut secrets = json!({
            "agent_key_hash": self.secrets.agent_key_hash,
            "channel_key": self.secrets.channel_key.as_ref().map(Secret::as_str),
            "hmac_secrets": self.secrets.hmac_secrets.iter().map(|(key, secret)| {
                json!([key, secret.as_str()])
            }).collect::<Vec<_>>(),
            "provider_credential": self.secrets.provider_credential.as_ref().map(Secret::as_str),
            "telegram_bot_token": self.secrets.telegram_bot_token.as_ref().map(Secret::as_str),
            "telegram_webhook_secret": self.secrets.telegram_webhook_secret.as_ref().map(Secret::as_str),
        });
        // Emitted only when present, so a legacy blob keeps its exact bytes and
        // an older image, which refuses the unknown key, can still read it.
        if let Some(credentials) = &self.secrets.native_lark {
            secrets["native_lark"] = credentials.private_json();
        }
        if let Some(credentials) = &self.secrets.whatsapp {
            secrets["whatsapp"] = credentials.private_json();
        }
        if let Some(token) = &self.secrets.discord_bot_token {
            secrets["discord_bot_token"] = json!(token.as_str());
        }
        if !self.secrets.consumer_keys.is_empty() {
            secrets["consumer_keys"] = self
                .secrets
                .consumer_keys
                .iter()
                .map(|(id, secret)| json!([id, secret.as_str()]))
                .collect();
        }
        if let Some(revisions) = &self.secrets.profile_revisions {
            secrets["profile_revisions"] = revisions.private_json();
        }
        if let Some(mutation) = &self.mutation {
            object.insert(
                "mutation".into(),
                serde_json::to_value(mutation).expect("mutation serializes"),
            );
        }
        object.insert("secrets".into(), secrets);
        Value::Object(object)
    }

    /// Serialize the public view, retaining only the hash needed for auth.
    pub fn to_public_json(&self) -> Value {
        let mut object = serde_json::to_value(self)
            .expect("channel definition is serializable without private fields")
            .as_object()
            .cloned()
            .expect("channel definition is an object");
        object.insert(
            "secrets".into(),
            json!({ "agent_key_hash": self.secrets.agent_key_hash }),
        );
        Value::Object(object)
    }
}
