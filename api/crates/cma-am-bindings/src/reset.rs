//! Validation of retained binding reset authority.

use crate::documents::binding_record;
use layer2_core::sandbox::binding::{ConversationResetState, ProviderBinding};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::Document;

pub const FIELD: &str = "conversation_resets_v1";
pub fn read_state(row: &Document) -> Result<ConversationResetState, SandboxStoreError> {
    read_state_for(&binding_record(row)?, row)
}

pub fn read_state_for(
    binding: &ProviderBinding,
    row: &Document,
) -> Result<ConversationResetState, SandboxStoreError> {
    let state: Option<ConversationResetState> = row
        .get(FIELD)
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()))
        .transpose()
        .map_err(|_| SandboxStoreError::Corrupt("invalid conversation reset state".into()))?;
    ConversationResetState::validate_format(binding, state.as_ref())
        .map_err(SandboxStoreError::Corrupt)?;
    Ok(state.unwrap_or_default())
}
