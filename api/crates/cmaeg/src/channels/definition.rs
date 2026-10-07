//! Build validated protocol definitions from management requests.

use std::collections::BTreeSet;

use cmaeg_core::{
    channel::{ChannelDefinition, ChannelSecrets, CustomAuth, ProviderBinding, Secret, Source},
    ids::{BindingId, RecordId},
    profile::{NyxIdIdentity, NyxIdIdentityKind},
    seal::ChannelPointer,
    ValidationError,
};
use ring::digest::{digest, SHA256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub use super::requests::*;

pub fn build_definition(
    request: &ChannelCreateRequest,
    owner: &str,
    record_id: RecordId,
    binding_id: BindingId,
    level: cmaeg_core::channel::ConformanceLevel,
) -> Result<ChannelDefinition, ValidationError> {
    let credentials = &request.credentials;
    if credentials.agent_key.as_str().is_empty() {
        return Err(ValidationError::new(
            "/credentials/agent_key",
            "must not be empty",
        ));
    }
    let definition = ChannelDefinition {
        schema_version: request.schema_version,
        name: request.name.clone(),
        record_id,
        creation: None,
        owner: NyxIdIdentity {
            kind: if request.profile.metadata.owner.kind == NyxIdIdentityKind::Organization {
                NyxIdIdentityKind::Organization
            } else {
                NyxIdIdentityKind::Human
            },
            subject: owner.to_owned(),
        },
        profile: request.profile.clone(),
        profile_digest: request.profile.digest(),
        provider: ProviderBinding {
            slug: request.provider.slug.clone(),
            target: request.provider.target.clone(),
            binding_id,
            level,
        },
        sources: build_sources(&request.sources)?,
        connection_ref: request.connection_ref.clone(),
        partition: request.partition.clone(),
        reply: request.reply.clone(),
        secrets: build_secrets(credentials),
        created_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|error| ValidationError::new("/created_at", error.to_string()))?,
        version: 0,
    };
    validate_definition(&definition)?;
    Ok(definition)
}

pub fn validate_create_request(request: &ChannelCreateRequest) -> Result<(), ValidationError> {
    request.provider.target.validate()?;
    if request.schema_version != cmaeg_core::channel::CHANNEL_SCHEMA_VERSION {
        return Err(ValidationError::new(
            "/schema_version",
            "unsupported schema version",
        ));
    }
    if request.name.is_empty() || request.name.len() > 80 {
        return Err(ValidationError::new("/name", "must contain 1..=80 bytes"));
    }
    request.profile.validate().map_err(|error| {
        ValidationError::new(format!("/profile{}", error.pointer), error.message)
    })?;
    build_sources(&request.sources)?;
    validate_credentials(&request.credentials)
}

pub fn validate_update_request(request: &ChannelUpdateRequest) -> Result<(), ValidationError> {
    request.provider.target.validate()?;
    if request.schema_version != cmaeg_core::channel::CHANNEL_SCHEMA_VERSION {
        return Err(ValidationError::new(
            "/schema_version",
            "unsupported schema version",
        ));
    }
    if request.name.is_empty() || request.name.len() > 80 {
        return Err(ValidationError::new("/name", "must contain 1..=80 bytes"));
    }
    request.profile.validate().map_err(|error| {
        ValidationError::new(format!("/profile{}", error.pointer), error.message)
    })?;
    build_sources(&request.sources)?;
    if let Some(credentials) = &request.credentials {
        if let Some(agent_key) = &credentials.agent_key {
            if agent_key.as_str().is_empty() {
                return Err(ValidationError::new(
                    "/credentials/agent_key",
                    "must not be empty",
                ));
            }
        }
        if let Some(channel_key) = &credentials.channel_key {
            if channel_key.as_str().is_empty() {
                return Err(ValidationError::new(
                    "/credentials/channel_key",
                    "must not be empty",
                ));
            }
        }
    }
    Ok(())
}

pub fn build_update(
    request: &ChannelUpdateRequest,
    current: &ChannelDefinition,
) -> Result<ChannelDefinition, ValidationError> {
    if request.provider.target != current.provider.target {
        return Err(ValidationError::new(
            "/provider/target",
            "target is immutable; create a new channel to change it",
        ));
    }
    if (request.profile.metadata.owner.kind == NyxIdIdentityKind::Organization)
        != (current.profile.metadata.owner.kind == NyxIdIdentityKind::Organization)
    {
        return Err(ValidationError::new(
            "/profile/metadata/owner/kind",
            "organization ownership is immutable",
        ));
    }
    let mut credentials = current.secrets.clone();
    if let Some(patch) = &request.credentials {
        if let Some(agent_key) = &patch.agent_key {
            if agent_key.as_str().is_empty() {
                return Err(ValidationError::new(
                    "/credentials/agent_key",
                    "must not be empty",
                ));
            }
            credentials.agent_key_hash = hash_secret(agent_key.as_str());
            credentials.provider_credential = Some(agent_key.clone());
        }
        if let Some(channel_key) = &patch.channel_key {
            credentials.channel_key = Some(channel_key.clone());
        }
        if let Some(token) = &patch.telegram_bot_token {
            credentials.telegram_bot_token = Some(token.clone());
        }
        if let Some(secret) = &patch.telegram_webhook_secret {
            credentials.telegram_webhook_secret = Some(secret.clone());
        }
        if let Some(hmac_secrets) = &patch.hmac_secrets {
            credentials.hmac_secrets = hmac_secrets
                .iter()
                .map(|secret| (secret.key_id.clone(), secret.secret.clone()))
                .collect();
        }
    }
    let definition = ChannelDefinition {
        schema_version: request.schema_version,
        name: request.name.clone(),
        record_id: current.record_id.clone(),
        creation: current.creation.clone(),
        owner: current.owner.clone(),
        profile: request.profile.clone(),
        profile_digest: request.profile.digest(),
        provider: current.provider.clone(),
        sources: build_sources(&request.sources)?,
        connection_ref: request.connection_ref.clone(),
        partition: request.partition.clone(),
        reply: request.reply.clone(),
        secrets: credentials,
        created_at: current.created_at.clone(),
        version: current.version,
    };
    validate_definition(&definition)?;
    Ok(definition)
}

fn build_sources(sources: &[SourceRequest]) -> Result<Vec<Source>, ValidationError> {
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
        if result.len() > 4 {
            return Err(ValidationError::new(pointer, "too many sources"));
        }
    }
    Ok(result)
}

fn build_secrets(credentials: &CredentialsRequest) -> ChannelSecrets {
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
    }
}

fn validate_credentials(credentials: &CredentialsRequest) -> Result<(), ValidationError> {
    for (pointer, value) in [
        ("/credentials/agent_key", Some(&credentials.agent_key)),
        ("/credentials/channel_key", credentials.channel_key.as_ref()),
    ] {
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
    }
    let mut keys = BTreeSet::new();
    for (index, secret) in credentials.hmac_secrets.iter().enumerate() {
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

pub fn hash_secret(secret: &str) -> String {
    let digest = digest(&SHA256, secret.as_bytes());
    format!(
        "sha256:{}",
        digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

pub fn channel_pointer(
    base_url: &str,
    record_id: &RecordId,
) -> Result<ChannelPointer, ValidationError> {
    let record_url = format!(
        "{}/api/v1/gateway/channel-records/{}",
        base_url.trim_end_matches('/'),
        record_id
    );
    let pointer = ChannelPointer {
        v: 1,
        record_url,
        record_id: record_id.clone(),
    };
    pointer.validate()?;
    Ok(pointer)
}

fn validate_definition(definition: &ChannelDefinition) -> Result<(), ValidationError> {
    definition.validate()?;
    if definition.profile.metadata.owner.subject != definition.owner.subject {
        return Err(ValidationError::new(
            "/profile/metadata/owner/subject",
            "must match the authenticated channel owner",
        ));
    }
    if definition.provider.slug != definition.profile.metadata.provider {
        return Err(ValidationError::new(
            "/provider/slug",
            "must match the profile provider",
        ));
    }
    let credentials = CredentialsRequest {
        agent_key: definition
            .secrets
            .provider_credential
            .clone()
            .unwrap_or_else(|| Secret::new("")),
        channel_key: definition.secrets.channel_key.clone(),
        telegram_bot_token: definition.secrets.telegram_bot_token.clone(),
        telegram_webhook_secret: definition.secrets.telegram_webhook_secret.clone(),
        hmac_secrets: definition
            .secrets
            .hmac_secrets
            .iter()
            .map(|(key_id, secret)| HmacSecretRequest {
                key_id: key_id.clone(),
                secret: secret.clone(),
            })
            .collect(),
    };
    validate_credentials(&credentials)?;
    if definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::NyxidRelay { .. }))
        && credentials
            .channel_key
            .as_ref()
            .is_none_or(|key| key.as_str().is_empty())
    {
        return Err(ValidationError::new(
            "/credentials/channel_key",
            "is required for a relay source",
        ));
    }
    for (index, source) in definition.sources.iter().enumerate() {
        if let Source::Custom {
            auth: CustomAuth::Hmac { key_id },
            ..
        } = source
        {
            if !definition
                .secrets
                .hmac_secrets
                .iter()
                .any(|(id, _)| id == key_id)
            {
                return Err(ValidationError::new(
                    format!("/sources/{index}/auth/key_id"),
                    "must reference a supplied HMAC secret",
                ));
            }
        }
    }
    Ok(())
}

impl ChannelCreateRequest {
    pub(crate) fn fingerprint(&self) -> String {
        let mut value = serde_json::json!({
            "record_id": self.record_id,
            "schema_version": self.schema_version, "name": self.name, "profile": self.profile,
            "provider": self.provider, "sources": self.sources, "partition": self.partition,
            "reply": self.reply, "agent_key": hash_secret(self.credentials.agent_key.as_str()),
            "channel_key": self.credentials.channel_key.as_ref().map(|key| hash_secret(key.as_str())),
            "hmac_secrets": self.credentials.hmac_secrets.iter().map(|value| (&value.key_id, hash_secret(value.secret.as_str()))).collect::<Vec<_>>(),
        });
        if let Some(reference) = &self.connection_ref {
            value["connection_ref"] = reference.clone().into();
        }
        if let Some(admission) = &self.connection_admission {
            value["connection_admission"] =
                serde_json::to_value(admission).expect("admission serializes");
        }
        // Omit absent additions so retained creation fingerprints stay stable.
        if let Some(token) = &self.credentials.telegram_bot_token {
            value["telegram_bot_token"] = hash_secret(token.as_str()).into();
        }
        if let Some(secret) = &self.credentials.telegram_webhook_secret {
            value["telegram_webhook_secret"] = hash_secret(secret.as_str()).into();
        }
        hash_secret(&cmaeg_core::profile::canonical_json(value))
    }
}
