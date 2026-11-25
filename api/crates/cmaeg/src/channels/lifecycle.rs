//! Channel delete and master-key rekey over sealed CMA records.

use super::management::{load, load_unowned, open_pointer, seal_definition};
use super::state::{ManagementError, ManagementState};
use crate::{auth::Creator, providers::ProviderError, records::RecordError};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::{ChannelDefinition, ChannelMutation, ChannelMutationKind},
    ids::ChannelId,
    ValidationError,
};

const RETIRE_UNREADY: &str = "retire_unready";
const UNCONFIRMED_CREATE: &str = "unconfirmed_create";

pub(crate) fn unready_retirement_version(definition: &ChannelDefinition) -> Option<u64> {
    definition
        .mutation
        .as_ref()
        .filter(|mutation| {
            mutation.operation == ChannelMutationKind::Delete
                && mutation.completed.iter().any(|step| step == RETIRE_UNREADY)
        })
        .map(|mutation| mutation.original_version)
}

/// Retain provider cleanup without allowing the original create to activate.
pub(crate) async fn retire_unready(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    let retirement_version = unready_retirement_version(&current.definition);
    let version = retirement_version.unwrap_or(current.record.version);
    if version != expected_version {
        return Err(ManagementError::Conflict {
            expected: expected_version,
            actual: Some(version),
        });
    }
    if current
        .definition
        .creation
        .as_ref()
        .is_none_or(|creation| creation.ready)
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/retire_unready",
            "only an incomplete channel may be retired",
        )));
    }
    if retirement_version.is_some() {
        return Ok(());
    }
    let mut definition = current.definition;
    let mut mutation = match definition.mutation.take() {
        Some(previous)
            if matches!(
                previous.operation,
                ChannelMutationKind::Create | ChannelMutationKind::Delete
            ) =>
        {
            previous
        }
        Some(_) => {
            return Err(ManagementError::Conflict {
                expected: expected_version,
                actual: Some(version),
            });
        }
        None => ChannelMutation {
            nonce: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            operation: ChannelMutationKind::Delete,
            original_version: version,
            fingerprint: String::new(),
            intended: STANDARD.encode(seal_definition(&state.master_keys, &definition)?),
            completed: Vec::new(),
            // Without custody, a different instance may still have a bind in flight.
            effect: Some(UNCONFIRMED_CREATE.into()),
        },
    };
    // Store the original provider effect and target. Change the operation so
    // readers reject activation even when their local custody switch is off.
    mutation.nonce = uuid::Uuid::new_v4().to_string();
    mutation.operation = ChannelMutationKind::Delete;
    mutation.original_version = version;
    mutation.fingerprint = format!("{version:064x}");
    mutation.completed.push(RETIRE_UNREADY.into());
    definition.mutation = Some(mutation);
    definition.validate().map_err(ManagementError::Invalid)?;
    let blob = seal_definition(&state.master_keys, &definition)?;
    let stored_version = match state
        .records
        .put(
            &definition.record_id,
            &creator.subject,
            &blob,
            Some(current.record.version),
        )
        .await
    {
        Ok(stored_version) => stored_version,
        Err(error) => {
            // A retry may have committed the same retirement under a prior request.
            let saved = load(state, creator, channel_id).await?;
            if unready_retirement_version(&saved.definition) != Some(expected_version) {
                return Err(error.into());
            }
            saved.record.version
        }
    };
    state
        .index
        .changed(&definition.record_id, &creator.subject, stored_version);
    state.resolver.invalidate(channel_id.as_str());
    Ok(())
}

pub async fn delete(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    if super::mutation::original_version(&current.definition, current.record.version)
        != expected_version
    {
        return Err(ManagementError::Conflict {
            expected: expected_version,
            actual: Some(current.record.version),
        });
    }
    state.resolver.invalidate(channel_id.as_str());
    let credential = current
        .definition
        .secrets
        .provider_credential
        .as_ref()
        .ok_or_else(|| {
            ManagementError::Provider(ProviderError::Unavailable(
                "channel provider credential is unavailable".into(),
            ))
        })?;
    let mut reservation = super::mutation::Reservation::acquire(
        &state.records,
        &state.master_keys,
        &current.definition,
        current.record.version,
        super::mutation::Intent {
            operation: cmaeg_core::channel::ChannelMutationKind::Delete,
            expected_version,
            fingerprint: format!("{:064x}", expected_version),
            desired: &current.definition,
        },
    )
    .await?;
    if current.definition.connection_ref.is_some() && !reservation.done("disconnect_connection") {
        reservation
            .effect(
                &state.records,
                &state.master_keys,
                "disconnect_connection".into(),
            )
            .await?;
        if let Err(error) = crate::connections::binding::disconnect_custodied(
            state,
            &current.definition,
            channel_id,
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
                reservation.desired.clone(),
            )
            .await?;
    }
    let step = format!(
        "unbind:{}:{}",
        current.definition.provider.slug, current.definition.provider.binding_id
    );
    if !reservation.done(&step) {
        reservation
            .effect(&state.records, &state.master_keys, step)
            .await?;
        if let Err(error) = state
            .providers
            .unbind_custodied(
                &current.definition.provider.slug,
                &current.definition.provider.binding_id,
                credential,
            )
            .await
        {
            if !error.unconfirmed {
                reservation
                    .retryable(&state.records, &state.master_keys)
                    .await?;
            }
            return Err(match error.error {
                // A definite answer that is not absence keeps the record (#1465).
                crate::providers::ProviderError::Rejected { status, code } => {
                    ManagementError::ReleaseRefused { status, code }
                }
                other => ManagementError::Provider(other),
            });
        }
        reservation
            .confirmed(
                &state.records,
                &state.master_keys,
                reservation.desired.clone(),
            )
            .await?;
    }
    reservation.delete(&state.records).await?;
    state.index.removed(&current.definition.record_id);
    crate::connections::binding::release(state, &current.definition).await?;
    state.resolver.invalidate(channel_id.as_str());
    Ok(())
}

pub async fn rekey(state: &ManagementState, channel_id: &ChannelId) -> Result<u8, ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let loaded = load_unowned(state, channel_id).await?;
    super::mutation::available(&loaded.definition, loaded.record.version)?;
    state.resolver.invalidate(channel_id.as_str());
    let mut definition = loaded.definition;
    definition.version = loaded.record.version;
    let blob = seal_definition(state.master_keys.as_ref(), &definition)?;
    let version = state
        .records
        .put(
            &definition.record_id,
            &definition.owner.subject,
            &blob,
            Some(loaded.record.version),
        )
        .await
        .map_err(|error| match error {
            RecordError::Conflict { expected, actual } => {
                ManagementError::Conflict { expected, actual }
            }
            other => ManagementError::Record(other),
        })?;
    state
        .index
        .changed(&definition.record_id, &definition.owner.subject, version);
    state.resolver.invalidate(channel_id.as_str());
    Ok(state.master_keys.current_kid())
}
