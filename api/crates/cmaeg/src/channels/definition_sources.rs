//! Build and validate the source list and secrets of a channel definition.

use std::collections::BTreeSet;

use cmaeg_core::{
    channel::{ChannelSecrets, CustomAuth, Secret, Source},
    ValidationError,
};

use super::definition::hash_secret;
use super::requests::{CredentialsRequest, HmacSecretRequest, SourceRequest};

pub(super) fn build_sources(
    sources: &[SourceRequest],
    limit: usize,
) -> Result<Vec<Source>, ValidationError> {
    let mut result = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let pointer = format!("/sources/{index}");
        if let SourceRequest::NyxidRelay {
            issuer,
            platform,
            trust_normalized,
            ..
        } = source
        {
            // Refuse up front: a stored source like this would admit no callbacks,
            // so a bot moved onto it would go silent. Ingress still refuses one as
            // defense in depth for definitions stored before this check existed.
            if !platform.verifies_raw_event() && !trust_normalized {
                return Err(ValidationError::new(
                    format!("{pointer}/trust_normalized"),
                    "the platform has no raw-event verifier; set trust_normalized to true to admit its normalized callbacks",
                ));
            }
            let parsed = reqwest::Url::parse(issuer).map_err(|_| {
                ValidationError::new(format!("{pointer}/issuer"), "must be an HTTPS origin")
            })?;
            if parsed.scheme() != "https"
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.path() != "/"
            {
                return Err(ValidationError::new(
                    format!("{pointer}/issuer"),
                    "must be an HTTPS origin",
                ));
            }
        }
        result.push(match source {
            SourceRequest::Telegram {
                bot_id,
                bot_username,
                admission,
            } => Source::Telegram {
                bot_id: bot_id.clone(),
                bot_username: bot_username.clone(),
                admission: admission.clone(),
            },
            SourceRequest::NyxidRelay {
                issuer,
                key_id,
                route_ids,
                platform,
                bot_username,
                bot_id,
                trust_normalized,
                admission,
            } => Source::NyxidRelay {
                issuer: issuer.clone(),
                jwks_url: format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/')),
                key_id: key_id.clone(),
                route_ids: route_ids.clone(),
                platform: *platform,
                bot_username: bot_username.clone(),
                bot_id: bot_id.clone(),
                trust_normalized: *trust_normalized,
                admission: admission.clone(),
            },
            SourceRequest::Custom {
                auth,
                prompt_template,
                schema,
                attachments,
            } => Source::Custom {
                auth: auth.clone(),
                prompt_template: prompt_template.clone(),
                schema: schema.clone(),
                attachments: attachments.clone(),
            },
        });
        if result.len() > limit {
            return Err(ValidationError::new(pointer, "too many sources"));
        }
    }
    Ok(result)
}

pub(super) fn build_secrets(credentials: &CredentialsRequest) -> ChannelSecrets {
    ChannelSecrets {
        agent_key_hash: hash_secret(credentials.agent_key.as_str()),
        channel_key: credentials.channel_key.clone(),
        hmac_secrets: credentials
            .hmac_secrets
            .iter()
            .map(|secret| (secret.key_id.clone(), secret.secret.clone()))
            .collect(),
        provider_credential: Some(credentials.agent_key.clone()),
        telegram_bot_token: credentials.telegram_bot_token.clone(),
        telegram_webhook_secret: credentials.telegram_webhook_secret.clone(),
        consumer_keys: Vec::new(),
    }
}

pub(super) fn validate_credentials(
    credentials: &CredentialsRequest,
) -> Result<(), ValidationError> {
    validate_bearer("/credentials/agent_key", Some(&credentials.agent_key))?;
    validate_source_credentials(credentials.channel_key.as_ref(), &credentials.hmac_secrets)
}

fn validate_bearer(pointer: &str, value: Option<&Secret>) -> Result<(), ValidationError> {
    if let Some(value) = value {
        if value.as_str().is_empty()
            || value.as_str().len() > 16_384
            || !value
                .as_str()
                .bytes()
                .all(|byte| (33..=126).contains(&byte))
        {
            return Err(ValidationError::new(
                pointer,
                "must contain 1..=16384 printable bearer bytes",
            ));
        }
    }
    Ok(())
}

/// Source-only edits do not require an unrelated provider credential.
pub(super) fn validate_source_credentials(
    channel_key: Option<&Secret>,
    hmac_secrets: &[HmacSecretRequest],
) -> Result<(), ValidationError> {
    validate_bearer("/credentials/channel_key", channel_key)?;
    let mut keys = BTreeSet::new();
    for (index, secret) in hmac_secrets.iter().enumerate() {
        if secret.key_id.is_empty() || secret.key_id.len() > 512 {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/key_id"),
                "must contain 1..=512 bytes",
            ));
        }
        if secret.secret.as_str().is_empty() || secret.secret.as_str().len() > 4_096 {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/secret"),
                "must contain 1..=4096 bytes",
            ));
        }
        use base64::Engine as _;
        if base64::engine::general_purpose::STANDARD
            .decode(secret.secret.as_str())
            .is_err()
        {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/secret"),
                "must be base64",
            ));
        }
        if !keys.insert(&secret.key_id) {
            return Err(ValidationError::new(
                "/credentials/hmac_secrets",
                "key ids must be unique",
            ));
        }
    }
    Ok(())
}

/// Writer requirements beyond the unconditional stored-shape reader rules.
pub(super) fn validate_source_requirements(
    sources: &[Source],
    secrets: &ChannelSecrets,
) -> Result<(), ValidationError> {
    if sources
        .iter()
        .any(|source| matches!(source, Source::NyxidRelay { .. }))
        && secrets
            .channel_key
            .as_ref()
            .is_none_or(|key| key.as_str().is_empty())
    {
        return Err(ValidationError::new(
            "/credentials/channel_key",
            "is required for a relay source",
        ));
    }
    for (index, source) in sources.iter().enumerate() {
        if let Source::Custom {
            auth: CustomAuth::Hmac { key_id },
            ..
        } = source
        {
            if !secrets.hmac_secrets.iter().any(|(id, _)| id == key_id) {
                return Err(ValidationError::new(
                    format!("/sources/{index}/auth/key_id"),
                    "must reference a supplied HMAC secret",
                ));
            }
        }
    }
    Ok(())
}
