//! Channel delete and master-key rekey over sealed CMA records.

use super::inspection::load_unowned;
use super::management::{inspect, load, open_pointer, seal_definition};
use super::state::{ManagementError, ManagementState};
use crate::{auth::Creator, providers::ProviderError, records::RecordError};
use cmaeg_core::{
    channel::ChannelMutationKind,
    ids::ChannelId,
    seal::{open_blob, seal_retired_channel},
    ValidationError,
};

/// Retain provider cleanup without allowing the original create to activate.
pub(crate) async fn retire_unready(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let (current, retirement_version) = inspect(state, creator, channel_id).await?;
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
    if current.definition.mutation.as_ref().is_some_and(|mutation| {
        !matches!(mutation.operation, ChannelMutationKind::Create | ChannelMutationKind::Delete)
    }) {
        return Err(ManagementError::Conflict {
            expected: expected_version,
            actual: Some(version),
        });
    }
    let successor = version.checked_add(1).ok_or_else(|| {
        ManagementError::Internal("channel record version exhausted".into())
    })?;
    let record_id = &current.definition.record_id;
    let opened = open_blob(&state.master_keys, &current.record.blob)
        .map_err(|_| ManagementError::Internal("channel blob cannot be opened".into()))?;
    // Retain the complete creation and cleanup state, including any tracked
    // effect and the unresolved possibility of an earlier untracked bind.
    // Ordinary readers refuse this seal kind before any provider or record write.
    let blob = seal_retired_channel(
        &state.master_keys,
        record_id.as_str(),
        successor,
        &opened.payload,
    )
    .map_err(|_| ManagementError::Internal("retired channel cannot be sealed".into()))?;
    let stored_version = match state
        .records
        .put(
            record_id,
            &creator.subject,
            &blob,
            Some(current.record.version),
        )
        .await
    {
        Ok(stored_version) => stored_version,
        Err(error) => {
            // A retry may have committed the same retirement under a prior request.
            let (saved, retirement) = inspect(state, creator, channel_id).await?;
            if retirement != Some(expected_version) {
                return Err(error.into());
            }
            saved.record.version
        }
    };
    state
        .index
        .changed(record_id, &creator.subject, stored_version);
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
