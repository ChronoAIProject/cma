//! Channel delete and master-key rekey over sealed CMA records.

use super::management::{load, load_by_record, load_unowned, open_pointer, seal_definition};
use super::state::{ManagementError, ManagementState};
use crate::{auth::Creator, records::RecordError};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, RecordId},
    ValidationError,
};

pub async fn delete(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, channel_id).await?;
    delete_owned(
        state,
        &current.definition,
        current.record.version,
        Some(channel_id),
        expected_version,
    )
    .await
}

pub async fn delete_legacy(
    state: &ManagementState,
    creator: &Creator,
    record_id: &RecordId,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let _guard = state.mutation_guard(record_id.as_str()).await;
    let current = load_by_record(state, creator, record_id).await?;
    if !current.valid || current.definition.creation.is_some() {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/record_id",
            "record retirement requires a valid legacy channel",
        )));
    }
    if !state.records.channel_mutation_custody() && current.definition.mutation.is_none() {
        return Err(ManagementError::Record(RecordError::Unavailable(
            "channel retirement requires mutation custody".into(),
        )));
    }
    delete_owned(
        state,
        &current.definition,
        current.version,
        None,
        expected_version,
    )
    .await
}

async fn delete_owned(
    state: &ManagementState,
    definition: &ChannelDefinition,
    version: u64,
    channel_id: Option<&ChannelId>,
    expected_version: u64,
) -> Result<(), ManagementError> {
    if super::mutation::original_version(definition, version) != expected_version {
        return Err(ManagementError::Conflict {
            expected: expected_version,
            actual: Some(version),
        });
    }
    let credential = definition.secrets.provider_credential.as_ref().ok_or_else(|| {
        if channel_id.is_none() {
            ManagementError::Invalid(ValidationError::new(
                "/record_id",
                "Provider cleanup needs the original channel credential. Contact the deployment operator.",
            ))
        } else {
            ManagementError::Internal("channel provider credential is unavailable".into())
        }
    })?;
    invalidate(state, definition, channel_id);
    let mut reservation = super::mutation::Reservation::acquire(
        &state.records,
        &state.master_keys,
        definition,
        version,
        super::mutation::Intent {
            operation: cmaeg_core::channel::ChannelMutationKind::Delete,
            expected_version,
            fingerprint: format!("{:064x}", expected_version),
            desired: definition,
        },
    )
    .await?;
    if channel_id.is_none() {
        tracing::info!(
            record = %crate::telemetry::channel_hash(definition.record_id.as_str()),
            expected_version,
            "Legacy channel retirement admitted"
        );
    }
    if definition.connection_ref.is_some() && !reservation.done("disconnect_connection") {
        reservation
            .effect(
                &state.records,
                &state.master_keys,
                "disconnect_connection".into(),
            )
            .await?;
        if let Err(error) =
            crate::connections::binding::disconnect_owned(state, definition, channel_id).await
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
        definition.provider.slug, definition.provider.binding_id
    );
    if !reservation.done(&step) {
        reservation
            .effect(&state.records, &state.master_keys, step)
            .await?;
        if let Err(error) = state
            .providers
            .unbind_custodied(
                &definition.provider.slug,
                &definition.provider.binding_id,
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
    state.index.removed(&definition.record_id);
    if channel_id.is_some() {
        crate::connections::binding::release(state, definition).await?;
    } else {
        crate::connections::binding::release_owned(state, definition, true).await?;
        tracing::info!(
            record = %crate::telemetry::channel_hash(definition.record_id.as_str()),
            expected_version,
            "Legacy channel retirement completed"
        );
    }
    invalidate(state, definition, channel_id);
    Ok(())
}

fn invalidate(
    state: &ManagementState,
    definition: &ChannelDefinition,
    channel_id: Option<&ChannelId>,
) {
    match channel_id {
        Some(id) => state.resolver.invalidate(id.as_str()),
        None => state.resolver.invalidate_record(&definition.record_id),
    }
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
