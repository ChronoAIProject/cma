//! Pure consumer projections and request-relative validation.
use super::{
    consumer_requests::{ConsumerCreateRequest, ConsumerUpdateRequest},
    definition::{hash_secret, validate_definition},
    management::seal_definition,
    state::{ManagementError, ManagementState},
};
use cmaeg_core::{
    channel::{ChannelDefinition, ConformanceLevel, Consumer, ProviderBinding, Secret},
    ids::{BindingId, ConsumerId},
    ValidationError,
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

pub(super) fn add(
    state: &ManagementState,
    current: &ChannelDefinition,
    request: ConsumerCreateRequest,
) -> Result<(ChannelDefinition, ConsumerId), ManagementError> {
    state
        .providers
        .validate_provider(&request.provider.slug)
        .map_err(ManagementError::Provider)?;
    let id = ConsumerId::parse(&format!(
        "csm_{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ))
    .map_err(ManagementError::Invalid)?;
    validate_key(current, &id, &request.credentials.agent_key)?;
    validate_name(current, &id, &request.name)?;
    validate_owner(current, &request.profile)?;
    let mut next = current.clone();
    let routing = next
        .routing
        .as_mut()
        .ok_or(ManagementError::InvalidReason("routing_required"))?;
    let index = routing.consumers.len();
    routing.consumers.push(Consumer {
        id: id.clone(),
        name: request.name,
        label: request.label,
        profile_digest: request.profile.digest(),
        profile: request.profile,
        provider: ProviderBinding {
            slug: request.provider.slug,
            target: request.provider.target,
            binding_id: BindingId::new(format!("bnd_{}", Uuid::new_v4().simple()))
                .map_err(ManagementError::Invalid)?,
            level: ConformanceLevel::L1,
        },
        agent_key_hash: hash_secret(request.credentials.agent_key.as_str()),
        mentionable: request.mentionable,
        non_human: request.non_human,
        reply: request.reply,
        publish: request.publish,
        pending: true,
        created_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| ManagementError::Internal("cannot format creation time".into()))?,
    });
    next.secrets
        .consumer_keys
        .push((id.clone(), request.credentials.agent_key));
    validate(
        state,
        current,
        &next,
        &format!("/routing/consumers/{index}"),
    )?;
    Ok((next, id))
}

pub(super) fn update(
    state: &ManagementState,
    current: &ChannelDefinition,
    id: &ConsumerId,
    request: ConsumerUpdateRequest,
) -> Result<ChannelDefinition, ManagementError> {
    let mut next = current.clone();
    let routing = next
        .routing
        .as_mut()
        .ok_or(ManagementError::InvalidReason("routing_required"))?;
    // Check presence, including explicit null: the default binding can only
    // be changed through the full channel update contract.
    let prefix = if id.is_default() {
        for (present, pointer) in [
            (request.profile.is_some(), "/profile"),
            (request.credentials.is_some(), "/credentials"),
        ] {
            if present {
                return Err(invalid(
                    pointer,
                    "default consumer binding changes require PUT /v1/channels",
                ));
            }
        }
        let consumer = &mut routing.default_consumer;
        consumer.name = request.name.clone();
        if let Some(label) = request.label {
            consumer.label = label;
        }
        if let Some(value) = request.mentionable {
            consumer.mentionable = value;
        }
        if let Some(value) = request.non_human {
            consumer.non_human = value;
        }
        if let Some(value) = request.reply {
            consumer.reply = value;
        }
        if let Some(value) = request.publish {
            consumer.publish = value;
        }
        "/routing/default_consumer".to_owned()
    } else {
        let index = routing
            .consumers
            .iter()
            .position(|consumer| &consumer.id == id)
            .ok_or(ManagementError::NotFound)?;
        let consumer = &mut routing.consumers[index];
        consumer.name = request.name.clone();
        if let Some(label) = request.label {
            consumer.label = label;
        }
        if let Some(profile) = request.profile {
            let profile = profile.ok_or_else(|| invalid("/profile", "must not be null"))?;
            validate_owner(current, &profile)?;
            consumer.profile_digest = profile.digest();
            consumer.profile = profile;
        }
        if let Some(credentials) = request.credentials {
            let credentials =
                credentials.ok_or_else(|| invalid("/credentials", "must not be null"))?;
            if let Some(key) = credentials.agent_key {
                validate_key(current, id, &key)?;
                consumer.agent_key_hash = hash_secret(key.as_str());
                let stored = next
                    .secrets
                    .consumer_keys
                    .iter_mut()
                    .find(|(stored, _)| stored == id)
                    .ok_or_else(|| {
                        ManagementError::Internal("missing consumer credential".into())
                    })?;
                stored.1 = key;
            }
        }
        if let Some(value) = request.mentionable {
            consumer.mentionable = value;
        }
        if let Some(value) = request.non_human {
            consumer.non_human = value;
        }
        if let Some(value) = request.reply {
            consumer.reply = value;
        }
        if let Some(value) = request.publish {
            consumer.publish = value;
        }
        format!("/routing/consumers/{index}")
    };
    validate_name(current, id, &request.name)?;
    validate(state, current, &next, &prefix)?;
    Ok(next)
}

pub(super) fn validate(
    state: &ManagementState,
    current: &ChannelDefinition,
    next: &ChannelDefinition,
    prefix: &str,
) -> Result<(), ManagementError> {
    validate_definition(Some(current), next, state.write_flags()).map_err(|mut error| {
        if let Some(suffix) = error.pointer.strip_prefix(&format!("{prefix}/")) {
            error.pointer = if suffix == "agent_key_hash" {
                "/credentials/agent_key".into()
            } else {
                format!("/{suffix}")
            };
        }
        ManagementError::Invalid(error)
    })?;
    seal_definition(&state.master_keys, next)?;
    Ok(())
}

fn validate_key(
    current: &ChannelDefinition,
    id: &ConsumerId,
    key: &Secret,
) -> Result<(), ManagementError> {
    if !(1..=4096).contains(&key.as_str().len())
        || !key.as_str().bytes().all(|byte| (33..=126).contains(&byte))
    {
        return Err(invalid(
            "/credentials/agent_key",
            "must contain 1..=4096 printable ASCII bytes without spaces",
        ));
    }
    let hash = hash_secret(key.as_str());
    if current
        .consumers()
        .any(|consumer| consumer.id != id && consumer.agent_key_hash == hash)
    {
        return Err(invalid("/credentials/agent_key", "consumer_key_reused"));
    }
    Ok(())
}

fn validate_name(
    current: &ChannelDefinition,
    id: &ConsumerId,
    name: &cmaeg_core::ids::ConsumerName,
) -> Result<(), ManagementError> {
    if current
        .consumers()
        .any(|consumer| consumer.id != id && consumer.name == name.as_str())
    {
        return Err(invalid(
            "/name",
            "must be a unique, unreserved consumer name",
        ));
    }
    Ok(())
}

fn validate_owner(
    current: &ChannelDefinition,
    profile: &cmaeg_core::profile::EventWorkerProfile,
) -> Result<(), ManagementError> {
    for (matches, pointer) in [
        (
            profile.metadata.owner.subject == current.owner.subject,
            "/profile/metadata/owner/subject",
        ),
        (
            profile.metadata.owner.kind == current.owner.kind,
            "/profile/metadata/owner/kind",
        ),
    ] {
        if !matches {
            return Err(invalid(
                pointer,
                "must match the authenticated channel owner",
            ));
        }
    }
    Ok(())
}

fn invalid(pointer: &str, reason: &str) -> ManagementError {
    ManagementError::Invalid(ValidationError::new(pointer, reason))
}
