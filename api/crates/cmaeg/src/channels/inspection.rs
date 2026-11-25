//! Owner inspection and mutation admission over sealed channel records.
use super::{
    definition::channel_pointer,
    management::open_pointer,
    state::{LoadedChannel, ManagementError, ManagementState, RecordedChannel},
};
use crate::{auth::Creator, records::RecordError};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, RecordId},
    seal::open_retained_channel,
};

pub async fn load(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
) -> Result<LoadedChannel, ManagementError> {
    let (loaded, retirement) = inspect(state, creator, channel_id).await?;
    require_not_retired(retirement, loaded.record.version)?;
    Ok(loaded)
}

pub(crate) async fn inspect(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
) -> Result<(LoadedChannel, Option<u64>), ManagementError> {
    let (loaded, retirement) = inspect_unowned(state, channel_id).await?;
    if loaded.definition.owner.subject != creator.subject
        || (loaded.definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization)
            != creator.organization
    {
        return Err(ManagementError::NotFound);
    }
    Ok((loaded, retirement))
}

/// The channel `record_id` stores, read for `creator` only: the
/// record-keyed counterpart of [`load`], with the same owner check, for the
/// console's summaries. Another owner's record is `NotFound`. A definition
/// that opens but fails validation is returned with `valid: false` so the
/// console can name it; nothing may act on such a definition.
pub async fn load_by_record(
    state: &ManagementState,
    creator: &Creator,
    record_id: &RecordId,
) -> Result<RecordedChannel, ManagementError> {
    let pointer =
        channel_pointer(&state.cma_base_url, record_id).map_err(ManagementError::Invalid)?;
    let record = state
        .records
        .get(&pointer.record_url)
        .await
        .map_err(|error| match error {
            RecordError::NotFound => ManagementError::NotFound,
            other => ManagementError::Record(other),
        })?;
    let (opened, retirement_expected_version) = open_retained_channel(
        &state.master_keys,
        record_id.as_str(),
        record.version,
        &record.blob,
    )
    .map_err(|_| ManagementError::Internal("channel blob cannot be opened".into()))?;
    let definition: ChannelDefinition = serde_json::from_slice(&opened.payload)
        .map_err(|_| ManagementError::Internal("channel definition cannot be read".into()))?;
    if definition.record_id != *record_id
        || definition.owner.subject != creator.subject
        || (definition.owner.kind == cmaeg_core::profile::NyxIdIdentityKind::Organization)
            != creator.organization
    {
        return Err(ManagementError::NotFound);
    }
    let valid = definition.validate().is_ok();
    Ok(RecordedChannel {
        definition,
        version: record.version,
        record_bytes: record.blob.len(),
        valid,
        retirement_expected_version,
    })
}

pub(super) async fn load_unowned(
    state: &ManagementState,
    channel_id: &ChannelId,
) -> Result<LoadedChannel, ManagementError> {
    let (loaded, retirement) = inspect_unowned(state, channel_id).await?;
    require_not_retired(retirement, loaded.record.version)?;
    Ok(loaded)
}

pub(super) fn require_not_retired(
    retirement: Option<u64>,
    stored_version: u64,
) -> Result<(), ManagementError> {
    if let Some(admitted_version) = retirement {
        return Err(ManagementError::Conflict {
            expected: admitted_version,
            actual: Some(stored_version),
        });
    }
    Ok(())
}

async fn inspect_unowned(
    state: &ManagementState,
    channel_id: &ChannelId,
) -> Result<(LoadedChannel, Option<u64>), ManagementError> {
    let pointer = open_pointer(state, channel_id)?;
    let record = state
        .records
        .get(&pointer.record_url)
        .await
        .map_err(|error| match error {
            RecordError::NotFound => ManagementError::NotFound,
            other => ManagementError::Record(other),
        })?;
    let (opened, retirement_expected_version) = open_retained_channel(
        &state.master_keys,
        pointer.record_id.as_str(),
        record.version,
        &record.blob,
    )
    .map_err(|error| {
        ManagementError::Internal(format!("channel blob cannot be opened: {error}"))
    })?;
    let definition: ChannelDefinition =
        serde_json::from_slice(&opened.payload).map_err(|error| {
            ManagementError::Internal(format!("channel definition is malformed: {error}"))
        })?;
    if definition.record_id != pointer.record_id {
        return Err(ManagementError::NotFound);
    }
    definition
        .validate()
        .map_err(|_| ManagementError::Internal("retained channel is invalid".into()))?;
    Ok((
        LoadedChannel {
            channel_id: channel_id.clone(),
            pointer,
            definition,
            record,
        },
        retirement_expected_version,
    ))
}
