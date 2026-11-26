//! Recoverable creation reserves an encrypted record before provider side effects.
use super::{
    definition::{build_definition, channel_pointer, ChannelCreateRequest},
    management::*,
};
use crate::{
    auth::Creator,
    providers::BindingResponse,
    records::{CreateConnectionClaim, RecordError},
};
use cmaeg_core::{
    channel::{ChannelCreation, ChannelDefinition, ChannelMutationKind, ConformanceLevel},
    ids::{BindingId, RecordId},
    seal::open_retained_channel,
    ValidationError,
};

pub(crate) const PENDING_DETAIL: &str = "channel_creation_pending: channel setup is incomplete. Complete the original channel creation request with its original record ID and settings, then refresh before editing producers or sending events.";

pub(super) fn require_ready(definition: &ChannelDefinition) -> Result<(), ManagementError> {
    if definition
        .creation
        .as_ref()
        .is_some_and(|creation| !creation.ready)
    {
        tracing::info!("Channel edit refused while original creation is incomplete");
        return Err(ManagementError::CreationPending);
    }
    Ok(())
}

pub(super) async fn create(
    state: &ManagementState,
    creator: &Creator,
    request: ChannelCreateRequest,
    record_id: RecordId,
    claim: Option<&CreateConnectionClaim>,
) -> Result<CreatedChannel, ManagementError> {
    custom_attachments_allowed(state, &request.sources)?;
    let uuid = uuid::Uuid::parse_str(record_id.as_str()).map_err(|_| {
        ManagementError::Invalid(ValidationError::new("/record_id", "must be a UUID"))
    })?;
    if uuid.to_string() != record_id.as_str() {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/record_id",
            "must be a canonical UUID",
        )));
    }
    let _guard = state.mutation_guard(record_id.as_str()).await;
    let fingerprint = request.fingerprint();
    let mut current = match load_record_for_create(state, creator, &record_id).await {
        Ok(current) => current,
        Err(ManagementError::NotFound) => {
            let binding_id = BindingId::new(format!("bnd_{}", uuid.simple()))
                .map_err(ManagementError::Invalid)?;
            let pointer = channel_pointer(&state.cma_base_url, &record_id)
                .map_err(ManagementError::Invalid)?;
            let channel_id = pointer
                .seal_id(&state.master_keys)
                .map_err(|_| ManagementError::Internal("cannot seal channel pointer".into()))?;
            let mut definition = build_definition(
                &request,
                &creator.subject,
                record_id.clone(),
                binding_id,
                ConformanceLevel::L1,
                state.write_flags(),
            )
            .map_err(ManagementError::Invalid)?;
            // Nothing is sent for an unknown provider, so its refusal must not
            // leave a reserved channel behind (#3068).
            for consumer in definition.consumers() {
                state
                    .providers
                    .require_allowlisted(&consumer.provider.slug)
                    .map_err(ManagementError::Provider)?;
            }
            super::github_sources::validate(state, None, &definition).await?;
            super::custom_execution::validate_providers(&state.providers, &definition)?;
            super::custom_execution::validate_media(state, None, &definition)?;
            super::native_discord::initialize(state, &mut definition)?;
            super::native_telegram::initialize(state, None, &mut definition)?;
            crate::native_lark::management::initialize(state, None, &mut definition)?;
            crate::native_lark::management::verify(state, None, &definition).await?;
            crate::whatsapp::management::initialize(state, &mut definition)?;
            crate::whatsapp::management::verify(state, None, &definition).await?;
            definition.creation = Some(ChannelCreation {
                channel_id: channel_id.clone(),
                fingerprint: fingerprint.clone(),
                ready: false,
            });
            let blob = seal_definition(&state.master_keys, &definition)?;
            match state
                .records
                .reserve_channel(&record_id, creator, &blob, claim)
                .await
            {
                Ok(version) => {
                    state.index.changed(&record_id, &creator.subject, version);
                    LoadedChannel {
                        channel_id,
                        pointer,
                        definition,
                        record: crate::records::RecordValue { blob, version },
                    }
                }
                Err(RecordError::Conflict { .. }) => {
                    load_record_for_create(state, creator, &record_id).await?
                }
                Err(error) => return Err(ManagementError::Record(error)),
            }
        }
        Err(error) => return Err(error),
    };
    let creation = current
        .definition
        .creation
        .as_ref()
        .ok_or(ManagementError::Conflict {
            expected: 0,
            actual: Some(current.record.version),
        })?;
    if creation.fingerprint != fingerprint {
        return Err(ManagementError::Conflict {
            expected: 0,
            actual: Some(current.record.version),
        });
    }
    if creation.ready {
        return if current.create_replayable() {
            Ok(completed(current))
        } else {
            Err(ManagementError::Conflict {
                expected: 0,
                actual: Some(current.record.version),
            })
        };
    }
    super::github_sources::validate(state, None, &current.definition).await?;
    super::custom_execution::validate_providers(&state.providers, &current.definition)?;
    let mut reservation = super::mutation::Reservation::acquire(
        &state.records,
        &state.master_keys,
        &current.definition,
        current.record.version,
        super::mutation::Intent {
            operation: ChannelMutationKind::Create,
            expected_version: super::mutation::original_version(
                &current.definition,
                current.record.version,
            ),
            fingerprint: fingerprint.clone(),
            desired: &current.definition,
        },
    )
    .await?;
    current.definition = reservation.desired.clone();
    let step = format!(
        "bind:{}:{}",
        current.definition.provider.slug, current.definition.provider.binding_id
    );
    let mut binding = BindingResponse {
        binding_id: current.definition.provider.binding_id.clone(),
        level: current.definition.provider.level,
        capabilities: current.definition.provider.capabilities.clone(),
        honoured: Vec::new(),
        refused: Vec::new(),
    };
    if !reservation.done(&step) {
        let provider_request = binding_request(state, &current.definition)?;
        reservation
            .effect(&state.records, &state.master_keys, step)
            .await?;
        binding = match state
            .providers
            .bind_custodied(
                &current.definition.provider.slug,
                &current.definition.provider.binding_id,
                &provider_request,
            )
            .await
        {
            Ok(binding) => binding,
            Err(error) => {
                if !error.unconfirmed {
                    reservation
                        .retryable(&state.records, &state.master_keys)
                        .await?;
                }
                return Err(ManagementError::Provider(error.error));
            }
        };
        current.definition.provider.level = binding.level;
        current.definition.provider.capabilities = binding.capabilities.clone();
        reservation
            .confirmed(
                &state.records,
                &state.master_keys,
                current.definition.clone(),
            )
            .await?;
    }
    if current.definition.connection_ref.is_some() && !reservation.done("activate_connection") {
        reservation
            .effect(
                &state.records,
                &state.master_keys,
                "activate_connection".into(),
            )
            .await?;
        if let Err(error) = crate::connections::binding::activate_custodied(
            state,
            &current.definition,
            &current.channel_id,
        )
        .await
        {
            if !error.unconfirmed {
                reservation
                    .retryable(&state.records, &state.master_keys)
                    .await?;
            }
            return Err(error.error);
        }
        reservation
            .confirmed(
                &state.records,
                &state.master_keys,
                current.definition.clone(),
            )
            .await?;
    }
    current.definition.creation.as_mut().unwrap().ready = true;
    let version = match reservation
        .publish(&state.records, &state.master_keys, &current.definition)
        .await
    {
        Ok((_, version)) => version,
        Err(error @ ManagementError::Record(RecordError::Conflict { .. })) => {
            // Another exact create may have published first. Read only: neither
            // a pending reservation nor a different request proves completion.
            let Ok(retained) = load_record(state, creator, &record_id).await else {
                // An unavailable confirmation cannot replace the known CAS
                // conflict with a provider outage or prove creation succeeded.
                return Err(error);
            };
            if retained.create_replayable()
                && retained
                    .definition
                    .creation
                    .as_ref()
                    .is_some_and(|value| value.fingerprint == fingerprint)
            {
                return Ok(completed(retained));
            }
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    state.index.changed(&record_id, &creator.subject, version);
    state.resolver.invalidate(current.channel_id.as_str());
    Ok(CreatedChannel {
        channel_id: current.channel_id,
        record_id,
        definition: current.definition,
        version,
        binding,
    })
}

fn completed(current: LoadedChannel) -> CreatedChannel {
    let binding = BindingResponse {
        capabilities: current.definition.provider.capabilities.clone(),
        binding_id: current.definition.provider.binding_id.clone(),
        level: current.definition.provider.level,
        honoured: Vec::new(),
        refused: Vec::new(),
    };
    CreatedChannel {
        channel_id: current.channel_id,
        record_id: current.definition.record_id.clone(),
        definition: current.definition,
        version: current.record.version,
        binding,
    }
}

/// Recover the exact retained channel identity using an owner-reserved record UUID.
pub async fn load_record(
    state: &ManagementState,
    creator: &Creator,
    id: &RecordId,
) -> Result<LoadedChannel, ManagementError> {
    inspect_record(state, creator, id)
        .await
        .map(|(loaded, _)| loaded)
}

async fn load_record_for_create(
    state: &ManagementState,
    creator: &Creator,
    id: &RecordId,
) -> Result<LoadedChannel, ManagementError> {
    let (loaded, retirement) = inspect_record(state, creator, id).await?;
    super::inspection::require_not_retired(retirement, loaded.record.version)?;
    Ok(loaded)
}

async fn inspect_record(
    state: &ManagementState,
    creator: &Creator,
    id: &RecordId,
) -> Result<(LoadedChannel, Option<u64>), ManagementError> {
    let pointer = channel_pointer(&state.cma_base_url, id).map_err(ManagementError::Invalid)?;
    let record = state
        .records
        .get(&pointer.record_url)
        .await
        .map_err(|error| match error {
            RecordError::NotFound => ManagementError::NotFound,
            other => ManagementError::Record(other),
        })?;
    let (opened, retirement) = open_retained_channel(
        &state.master_keys,
        id.as_str(),
        record.version,
        &record.blob,
    )
    .map_err(|_| ManagementError::Internal("channel blob cannot be opened".into()))?;
    let definition: ChannelDefinition = serde_json::from_slice(&opened.payload)
        .map_err(|_| ManagementError::Internal("channel definition cannot be read".into()))?;
    if definition.owner.subject != creator.subject
        || definition.record_id != *id
        || (definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization)
            != creator.organization
    {
        return Err(ManagementError::NotFound);
    }
    definition
        .validate()
        .map_err(|_| ManagementError::Internal("retained channel is invalid".into()))?;
    let creation = definition
        .creation
        .as_ref()
        .ok_or(ManagementError::NotFound)?;
    let recovered =
        cmaeg_core::seal::ChannelPointer::open_id(&creation.channel_id, &state.master_keys)
            .map_err(|_| ManagementError::NotFound)?;
    if recovered.payload != pointer {
        return Err(ManagementError::NotFound);
    }
    Ok((
        LoadedChannel {
            channel_id: creation.channel_id.clone(),
            pointer,
            definition,
            record,
        },
        retirement,
    ))
}
