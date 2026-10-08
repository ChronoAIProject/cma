//! Owner management may address a channel by its public id or stored record.
use super::{
    definition::channel_pointer,
    management::open_pointer,
    state::{ManagementError, ManagementState},
};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, RecordId},
    seal::ChannelPointer,
    ValidationError,
};

#[derive(Debug, Clone)]
pub enum ChannelTarget {
    Id(ChannelId),
    Record(RecordId),
}

pub(crate) fn open_target(
    state: &ManagementState,
    target: &ChannelTarget,
) -> Result<ChannelPointer, ManagementError> {
    match target {
        ChannelTarget::Id(id) => open_pointer(state, id),
        ChannelTarget::Record(record_id) => {
            // The record client constrains UUIDs and the trusted origin/path;
            // generic sealed-pointer validation alone permits wider URLs.
            let url = state
                .records
                .record_url(record_id.as_str())
                .map_err(|_| invalid_pointer())?;
            let pointer =
                channel_pointer(&state.cma_base_url, record_id).map_err(|_| invalid_pointer())?;
            if pointer.record_url != url.as_str() {
                return Err(invalid_pointer());
            }
            Ok(pointer)
        }
    }
}

pub(super) fn loaded_id(
    state: &ManagementState,
    target: &ChannelTarget,
    pointer: &ChannelPointer,
    definition: &ChannelDefinition,
) -> Result<ChannelId, ManagementError> {
    if let ChannelTarget::Id(id) = target {
        return Ok(id.clone());
    }
    if let Some(creation) = &definition.creation {
        let retained = ChannelPointer::open_id(&creation.channel_id, &state.master_keys)
            .map_err(|_| invalid_pointer())?;
        if retained.payload != *pointer {
            return Err(invalid_pointer());
        }
        return Ok(creation.channel_id.clone());
    }
    // A connection binds its webhook to the retained creation identity. A
    // replacement id would be a different identity to its retirement fence.
    if definition.connection_ref.is_some() {
        return Err(invalid_pointer());
    }
    pointer
        .seal_id(&state.master_keys)
        .map_err(|_| ManagementError::Internal("cannot seal channel pointer".into()))
}

fn invalid_pointer() -> ManagementError {
    ManagementError::Invalid(ValidationError::new(
        "/channel_id",
        "malformed channel pointer",
    ))
}
