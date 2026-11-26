//! Build validated protocol definitions from management requests.

use cmaeg_core::{
    channel::{ChannelDefinition, CustomAuth, ProviderBinding, Secret, Source},
    ids::{BindingId, RecordId},
    profile::{NyxIdIdentity, NyxIdIdentityKind},
    seal::ChannelPointer,
    ValidationError,
};
use ring::digest::{digest, SHA256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::definition_sources::{build_secrets, build_sources, validate_credentials};
pub use super::requests::*;
use super::write_gate::{self, WriteFlags};

pub(super) fn project_default_profile(
    definition: &mut ChannelDefinition,
    view: cmaeg_core::channel::ProfileRevisionView<'_>,
) {
    definition.provider = view.provider.clone();
    definition.profile_digest = view.profile_digest.to_owned();
    definition.profile = view.profile;
    definition.secrets.provider_credential = Some(view.credential.clone());
}

pub fn build_definition(
    request: &ChannelCreateRequest,
    owner: &str,
    record_id: RecordId,
    binding_id: BindingId,
    level: cmaeg_core::channel::ConformanceLevel,
    flags: WriteFlags,
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
        mutation: None,
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
            capabilities: Vec::new(),
            slug: request.provider.slug.clone(),
            target: request.provider.target.clone(),
            binding_id,
            level,
        },
        sources: build_sources(&request.sources, None)?,
        connection_ref: request.connection_ref.clone(),
        partition: request.partition.clone(),
        reply: request.reply.clone(),
        routing: request
            .durable_custom_execution
            .then(super::custom_execution::default_routing),
        durable_custom_execution: request.durable_custom_execution,
        secrets: build_secrets(credentials),
        created_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|error| ValidationError::new("/created_at", error.to_string()))?,
        version: 0,
    };
    validate_definition(None, &definition, flags)?;
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
    build_sources(&request.sources, None)?;
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
    build_sources(&request.sources, None)?;
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
    flags: WriteFlags,
) -> Result<ChannelDefinition, ValidationError> {
    if request
        .durable_custom_execution
        .is_some_and(|value| value != current.durable_custom_execution)
    {
        return Err(ValidationError::new(
            "/durable_custom_execution",
            "immutable; create a new channel to change execution mode",
        ));
    }
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
    if current.secrets.profile_revisions.is_some()
        && (request.profile != current.profile
            || request.partition != current.partition
            || request
                .credentials
                .as_ref()
                .is_some_and(|patch| patch.agent_key.is_some()))
    {
        return Err(ValidationError::new(
            "/profile",
            "use profile revision publication to change a retained worker profile",
        ));
    }
    let mut credentials = current.secrets.clone();
    if let Some(patch) = &request.credentials {
        if let Some(value) = &patch.native_lark {
            credentials.native_lark = Some(value.clone());
        }
        if let Some(whatsapp) = &patch.whatsapp {
            credentials.whatsapp = Some(whatsapp.clone());
        }
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
        if let Some(token) = &patch.discord_bot_token {
            credentials.discord_bot_token = Some(token.clone());
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
    let sources = build_sources(&request.sources, Some(current))?;
    if !sources.iter().any(|s| matches!(s, Source::NativeLark(_))) {
        credentials.native_lark = None;
    }
    if !sources
        .iter()
        .any(|source| matches!(source, Source::Whatsapp { .. }))
    {
        credentials.whatsapp = None;
    }
    if current
        .sources
        .iter()
        .any(|source| matches!(source, Source::Discord { .. }))
        && !sources
            .iter()
            .any(|source| matches!(source, Source::Discord { .. }))
    {
        credentials.discord_bot_token = None;
    }
    let definition = ChannelDefinition {
        schema_version: request.schema_version,
        name: request.name.clone(),
        record_id: current.record_id.clone(),
        creation: current.creation.clone(),
        mutation: current.mutation.clone(),
        owner: current.owner.clone(),
        profile: request.profile.clone(),
        profile_digest: request.profile.digest(),
        provider: current.provider.clone(),
        sources,
        connection_ref: request.connection_ref.clone(),
        partition: request.partition.clone(),
        reply: request.reply.clone(),
        // No request carries routing yet, so a full update keeps the stored
        // policy, exactly as it keeps the consumer keys in the secrets.
        routing: current.routing.clone(),
        durable_custom_execution: current.durable_custom_execution,
        secrets: credentials,
        created_at: current.created_at.clone(),
        version: current.version,
    };
    validate_definition(Some(current), &definition, flags)?;
    Ok(definition)
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

/// Validate a definition a management write would store, replacing `current`
/// (`None` for a create). The write gate runs first, so a write it refuses
/// reports the legacy error it always has before any check below can answer.
pub(super) fn validate_definition(
    current: Option<&ChannelDefinition>,
    definition: &ChannelDefinition,
    flags: WriteFlags,
) -> Result<(), ValidationError> {
    write_gate::check(current, definition, flags)?;
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
    validate_consumer_owners(definition)?;
    validate_consumer_keys(definition)?;
    let credentials = CredentialsRequest {
        native_lark: definition.secrets.native_lark.clone(),
        whatsapp: definition.secrets.whatsapp.clone(),
        agent_key: definition
            .secrets
            .provider_credential
            .clone()
            .unwrap_or_else(|| Secret::new("")),
        channel_key: definition.secrets.channel_key.clone(),
        telegram_bot_token: definition.secrets.telegram_bot_token.clone(),
        discord_bot_token: definition.secrets.discord_bot_token.clone(),
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

/// Every additional consumer acts for the channel owner, as `default` does.
fn validate_consumer_owners(definition: &ChannelDefinition) -> Result<(), ValidationError> {
    let consumers = definition
        .routing
        .iter()
        .flat_map(|routing| &routing.consumers);
    for (index, consumer) in consumers.enumerate() {
        if consumer.profile.metadata.owner.subject != definition.owner.subject {
            return Err(ValidationError::new(
                format!("/routing/consumers/{index}/profile/metadata/owner/subject"),
                "must match the authenticated channel owner",
            ));
        }
    }
    Ok(())
}

/// Each additional consumer's sealed key must hash to its public
/// `agent_key_hash`. Tool calls authenticate a consumer by that hash while
/// the provider is handed the key, so a mismatched pair would admit one key
/// and bind another. Readers refuse such a definition as well as writers.
pub(crate) fn validate_consumer_keys(
    definition: &ChannelDefinition,
) -> Result<(), ValidationError> {
    let consumers = definition
        .routing
        .iter()
        .flat_map(|routing| &routing.consumers);
    for (index, consumer) in consumers.enumerate() {
        let matches = definition
            .secrets
            .consumer_keys
            .iter()
            .find(|(id, _)| id == &consumer.id)
            .is_some_and(|(_, key)| hash_secret(key.as_str()) == consumer.agent_key_hash);
        if !matches {
            return Err(ValidationError::new(
                format!("/routing/consumers/{index}/agent_key_hash"),
                "does not match the consumer's agent key",
            ));
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
        if self.durable_custom_execution {
            value["durable_custom_execution"] = true.into();
        }
        if let Some(reference) = &self.connection_ref {
            value["connection_ref"] = reference.clone().into();
        }
        if let Some(admission) = &self.connection_admission {
            value["connection_admission"] =
                serde_json::to_value(admission).expect("admission serializes");
        }
        if let Some(c) = &self.credentials.native_lark {
            value["native_lark"] = serde_json::json!({"app_secret":hash_secret(c.app_secret.as_str()),"encrypt_key":hash_secret(c.encrypt_key.as_str()),"verification_token":hash_secret(c.verification_token.as_str())});
        }
        if let Some(credentials) = &self.credentials.whatsapp {
            value["whatsapp"] = serde_json::json!({"app_secret": hash_secret(credentials.app_secret.as_str()), "access_token": hash_secret(credentials.access_token.as_str()), "verify_token": hash_secret(credentials.verify_token.as_str())});
        }
        // Omit absent additions so retained creation fingerprints stay stable.
        if let Some(token) = &self.credentials.discord_bot_token {
            value["discord_bot_token"] = hash_secret(token.as_str()).into();
        }
        if let Some(token) = &self.credentials.telegram_bot_token {
            value["telegram_bot_token"] = hash_secret(token.as_str()).into();
        }
        if let Some(secret) = &self.credentials.telegram_webhook_secret {
            value["telegram_webhook_secret"] = hash_secret(secret.as_str()).into();
        }
        hash_secret(&cmaeg_core::profile::canonical_json(value))
    }
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;

/// Stored-definition construction and mutation for service test fixtures.
#[cfg(test)]
pub(crate) mod fixture {
    use super::{hash_secret, BindingId, ChannelDefinition, Secret};

    pub(crate) fn with_agent_key(agent_key: &str) -> ChannelDefinition {
        let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        replace_default_agent_key(&mut definition, agent_key);
        definition.profile_digest = definition.profile.digest();
        definition
    }

    /// Model a stored default credential rotation for recovery fixtures.
    pub(crate) fn replace_default_agent_key(definition: &mut ChannelDefinition, agent_key: &str) {
        definition.secrets.agent_key_hash = hash_secret(agent_key);
        definition.secrets.provider_credential = Some(Secret::new(agent_key));
    }

    /// Change only the stored serving credential for rotation/revocation fixtures.
    pub(crate) fn replace_default_credential(
        definition: &mut ChannelDefinition,
        credential: Option<&str>,
    ) {
        definition.secrets.provider_credential = credential.map(Secret::new);
    }

    /// Model a stored binding replacement to test revocation of old authority.
    pub(crate) fn replace_default_binding(
        definition: &mut ChannelDefinition,
        binding_id: BindingId,
    ) {
        definition.provider.binding_id = binding_id;
    }
}
