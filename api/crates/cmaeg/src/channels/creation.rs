//! Recoverable creation reserves an encrypted record before provider side effects.
use super::{
    definition::{build_definition, channel_pointer, ChannelCreateRequest},
    management::*,
};
use crate::{auth::Creator, providers::BindingResponse, records::RecordError};
use cmaeg_core::{
    channel::{ChannelCreation, ChannelDefinition, ConformanceLevel},
    ids::{BindingId, RecordId},
    seal::open_blob,
    ValidationError,
};

pub async fn create(
    state: &ManagementState,
    creator: &Creator,
    request: ChannelCreateRequest,
    record_id: RecordId,
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
    let mut current = match load_record(state, creator, &record_id).await {
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
            )
            .map_err(ManagementError::Invalid)?;
            definition.creation = Some(ChannelCreation {
                channel_id: channel_id.clone(),
                fingerprint: fingerprint.clone(),
                ready: false,
            });
            let blob = seal_definition(&state.master_keys, &definition)?;
            match state
                .records
                .put(&record_id, &creator.subject, &blob, None)
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
                    load_record(state, creator, &record_id).await?
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
        return Ok(completed(current));
    }
    // Rebinding is idempotent in the provider contract. The retained record lets
    // the creator recover or delete this exact reservation after any uncertain result.
    let binding = state
        .providers
        .bind(
            &current.definition.provider.slug,
            &current.definition.provider.binding_id,
            &binding_request(state, &current.definition)?,
        )
        .await
        .map_err(ManagementError::Provider)?;
    current.definition.provider.level = binding.level;
    crate::connections::binding::activate(state, &current.definition, &current.channel_id).await?;
    current.definition.creation.as_mut().unwrap().ready = true;
    let blob = seal_definition(&state.master_keys, &current.definition)?;
    let version = match state
        .records
        .put(
            &record_id,
            &creator.subject,
            &blob,
            Some(current.record.version),
        )
        .await
    {
        Ok(version) => {
            state.index.changed(&record_id, &creator.subject, version);
            version
        }
        Err(RecordError::Conflict { .. }) => {
            let retained = load_record(state, creator, &record_id).await?;
            if retained
                .definition
                .creation
                .as_ref()
                .is_some_and(|value| value.ready && value.fingerprint == fingerprint)
            {
                return Ok(completed(retained));
            }
            return Err(ManagementError::Conflict {
                expected: current.record.version,
                actual: Some(retained.record.version),
            });
        }
        Err(error) => return Err(ManagementError::Record(error)),
    };
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
    let pointer = channel_pointer(&state.cma_base_url, id).map_err(ManagementError::Invalid)?;
    let record = state
        .records
        .get(&pointer.record_url)
        .await
        .map_err(|error| match error {
            RecordError::NotFound => ManagementError::NotFound,
            other => ManagementError::Record(other),
        })?;
    let opened = open_blob(&state.master_keys, &record.blob)
        .map_err(|_| ManagementError::Internal("channel blob cannot be opened".into()))?;
    let definition: ChannelDefinition = serde_json::from_slice(&opened.payload)
        .map_err(|_| ManagementError::Internal("channel definition cannot be read".into()))?;
    if definition.owner.subject != creator.subject || definition.record_id != *id {
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
    Ok(LoadedChannel {
        channel_id: creation.channel_id.clone(),
        pointer,
        definition,
        record,
    })
}
