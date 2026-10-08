//! Credentials supplied by a source attachment and retained-source ownership.
use super::{definition_sources::validate_source_credentials, HmacSecretRequest};
use cmaeg_core::{
    channel::{ChannelDefinition, ChannelSecrets, CustomAuth, Platform, Secret, Sink, Source},
    ValidationError,
};
use serde::Deserialize;
use std::collections::BTreeSet;
use utoipa::ToSchema;

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceCredentials {
    #[serde(default)]
    pub channel_key: Option<Secret>,
    #[serde(default)]
    pub telegram_bot_token: Option<Secret>,
    #[serde(default)]
    pub telegram_webhook_secret: Option<Secret>,
    #[serde(default)]
    pub hmac_secrets: Vec<HmacSecretRequest>,
}

pub(super) fn merge(
    current: &ChannelDefinition,
    supplied: &SourceCredentials,
) -> Result<ChannelSecrets, ValidationError> {
    let mut key_ids: BTreeSet<&str> = current
        .secrets
        .hmac_secrets
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    for (index, secret) in supplied.hmac_secrets.iter().enumerate() {
        if !key_ids.insert(&secret.key_id) {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/key_id"),
                "key id already exists",
            ));
        }
    }
    validate_source_credentials(supplied.channel_key.as_ref(), &supplied.hmac_secrets)?;
    if current
        .sources
        .iter()
        .any(|source| matches!(source, Source::NyxidRelay { .. }))
        && supplied
            .channel_key
            .as_ref()
            .is_some_and(|key| Some(key) != current.secrets.channel_key.as_ref())
    {
        return Err(ValidationError::new(
            "/credentials/channel_key",
            "relay sources share the channel key",
        ));
    }
    let mut next = current.secrets.clone();
    if let Some(key) = &supplied.channel_key {
        next.channel_key = Some(key.clone());
    }
    if let Some(token) = &supplied.telegram_bot_token {
        next.telegram_bot_token = Some(token.clone());
    }
    if let Some(secret) = &supplied.telegram_webhook_secret {
        next.telegram_webhook_secret = Some(secret.clone());
    }
    next.hmac_secrets.extend(
        supplied
            .hmac_secrets
            .iter()
            .map(|entry| (entry.key_id.clone(), entry.secret.clone())),
    );
    Ok(next)
}

pub(super) fn prune(next: &mut ChannelDefinition) {
    if !next
        .sources
        .iter()
        .any(|source| matches!(source, Source::NyxidRelay { .. }))
    {
        next.secrets.channel_key = None;
    }
    if next.connection_ref.is_none()
        && !next.sources.iter().any(|source| {
            matches!(
                source,
                Source::Telegram { .. }
                    | Source::NyxidRelay {
                        platform: Platform::Telegram,
                        ..
                    }
            )
        })
    {
        next.secrets.telegram_bot_token = None;
        next.secrets.telegram_webhook_secret = None;
    }
    let hmac: BTreeSet<&str> = next
        .sources
        .iter()
        .filter_map(|source| match source {
            Source::Custom {
                auth: CustomAuth::Hmac { key_id },
                ..
            } => Some(key_id.as_str()),
            _ => None,
        })
        .chain(next.reply.sinks.iter().filter_map(|sink| match sink {
            Sink::ResultWebhook { hmac_key_id, .. } => Some(hmac_key_id.as_str()),
            _ => None,
        }))
        .collect();
    next.secrets
        .hmac_secrets
        .retain(|(id, _)| hmac.contains(id.as_str()));
}
