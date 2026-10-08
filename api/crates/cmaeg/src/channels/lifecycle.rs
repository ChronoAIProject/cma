//! Channel delete and master-key rekey over sealed CMA records.

use super::management::{load, load_unowned, seal_definition};
use super::state::{InvalidationPath, ManagementError, ManagementState};
use super::target::{open_target, ChannelTarget};
use crate::{auth::Creator, records::RecordError};

pub async fn delete(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    expected_version: u64,
) -> Result<(), ManagementError> {
    let pointer = open_target(state, target)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let current = load(state, creator, target).await?;
    if current.record.version != expected_version {
        return Err(ManagementError::Conflict {
            expected: expected_version,
            actual: Some(current.record.version),
        });
    }
    crate::connections::binding::disconnect(state, &current.definition, &current.channel_id)
        .await?;
    state.invalidate_record(
        &pointer.record_id,
        Some(current.channel_id.as_str()),
        InvalidationPath::Delete,
    );
    let credential = current
        .definition
        .secrets
        .provider_credential
        .as_ref()
        .ok_or_else(|| {
            ManagementError::Internal("channel provider credential is unavailable".into())
        })?;
    state
        .providers
        .unbind(
            &current.definition.provider.slug,
            &current.definition.provider.binding_id,
            credential,
        )
        .await
        .map_err(ManagementError::Provider)?;
    match state
        .records
        .delete(&current.definition.record_id, expected_version)
        .await
    {
        Ok(true) | Ok(false) => {
            // Revocation must survive a failure while retiring the connection.
            state.invalidate_record(
                &pointer.record_id,
                Some(current.channel_id.as_str()),
                InvalidationPath::Delete,
            );
            crate::connections::binding::release(state, &current.definition).await?;
            Ok(())
        }
        Err(RecordError::Conflict { expected, actual }) => {
            Err(ManagementError::Conflict { expected, actual })
        }
        Err(error) => Err(ManagementError::Record(error)),
    }
}

pub async fn rekey(state: &ManagementState, target: &ChannelTarget) -> Result<u8, ManagementError> {
    let pointer = open_target(state, target)?;
    let _guard = state.mutation_guard(pointer.record_id.as_str()).await;
    let loaded = load_unowned(state, target).await?;
    state.invalidate_record(
        &pointer.record_id,
        Some(loaded.channel_id.as_str()),
        InvalidationPath::Rekey,
    );
    let mut definition = loaded.definition;
    definition.version = loaded.record.version;
    let blob = seal_definition(state.master_keys.as_ref(), &definition)?;
    state
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
    state.invalidate_record(
        &pointer.record_id,
        Some(loaded.channel_id.as_str()),
        InvalidationPath::Rekey,
    );
    Ok(state.master_keys.current_kid())
}
