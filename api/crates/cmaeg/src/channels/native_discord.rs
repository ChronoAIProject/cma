//! Default-only routing for newly created, explicitly authored native sources.
use super::management::{ManagementError, ManagementState};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};

pub(super) fn initialize(
    state: &ManagementState,
    definition: &mut ChannelDefinition,
) -> Result<(), ManagementError> {
    if !state.native_discord.discovery_enabled()
        || !definition
            .sources
            .iter()
            .any(|source| matches!(source, Source::Discord { .. }))
    {
        return Ok(());
    }
    if !state.native_discord_writes_enabled || !state.multi_consumer_writes_enabled {
        return Err(ManagementError::Invalid(ValidationError::new("/sources", "native Discord self-service creation requires native source and routing authoring support")));
    }
    if !state
        .providers
        .supports_response_replay(&definition.provider.slug)
        .unwrap_or(false)
    {
        return Err(ManagementError::Invalid(ValidationError::new(
            "/provider/slug",
            "native Discord requires a provider configured for durable response recovery",
        )));
    }
    // The implicit consumer retains its original binding, profile and credential.
    definition.routing = Some(
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "default_consumer": {"name": "default"},
            "consumers": [],
            "default_targets": ["default"]
        }))
        .map_err(|_| ManagementError::Internal("cannot initialize native routing".into()))?,
    );
    super::definition::validate_definition(None, definition, state.write_flags())
        .map_err(ManagementError::Invalid)
}
