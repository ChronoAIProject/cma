//! Owner-visible runtime eligibility; selection is not a socket-health claim.
use crate::channels::management::ManagementState;
use cmaeg_core::channel::{ChannelDefinition, Source};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativeDiscordActivation {
    pub state: String,
    pub detail: String,
    pub capacity: usize,
    pub selected_records: usize,
}
pub(crate) fn view(
    state: &ManagementState,
    definition: &ChannelDefinition,
) -> Option<NativeDiscordActivation> {
    if !definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::Discord { .. }))
    {
        return None;
    }
    let selection = &state.native_discord;
    let records = selection.records();
    let (status, detail) = if !selection.enabled() {
        (
            "disabled",
            "Native Discord intake is disabled on this gateway.",
        )
    } else if !definition
        .creation
        .as_ref()
        .is_some_and(|creation| creation.ready)
    {
        (
            "creation_pending",
            "Finish creating this channel before Discord can activate. Retry the original create request to resume it.",
        )
    } else if definition.routing.is_none() {
        (
            "routing_required",
            "Configure a routing policy before native Discord can activate.",
        )
    } else if !definition
        .consumers()
        .filter(|consumer| !consumer.pending)
        .all(|consumer| {
            state
                .providers
                .supports_response_replay(&consumer.provider.slug)
                .unwrap_or(false)
        })
    {
        (
            "provider_unavailable",
            "Every active consumer needs a provider configured for durable response recovery.",
        )
    } else if !state.native_discord_admission_enabled {
        (
            "admissions_paused",
            "Native Discord recovery is enabled, but new message admissions are paused.",
        )
    } else if records.contains(definition.record_id.as_str()) {
        ("selected", "Selected for native Discord intake. Bot authentication, connection health and external delivery are not confirmed by this status.")
    } else if !selection.discovery_enabled() {
        ("not_selected", "Automatic native Discord activation is disabled; this channel is outside the configured canary selection.")
    } else if records.len() >= super::selection::MAX_RECORDS {
        ("capacity_exceeded", "Native Discord activation capacity is full on this gateway instance. This channel is saved but is not selected here; other instances may differ. Free capacity or contact the gateway operator.")
    } else {
        ("pending_discovery", "Saved and awaiting automatic Discord activation. Refresh this channel or run Check configuration to confirm selection before testing delivery.")
    };
    Some(NativeDiscordActivation {
        state: status.into(),
        detail: detail.into(),
        capacity: super::selection::MAX_RECORDS,
        selected_records: records.len(),
    })
}
