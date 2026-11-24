//! Public endpoint URLs for a verified retained channel address.
use super::management::EndpointResponse;
use crate::channels::management::ManagementState;
use cmaeg_core::{channel::ChannelDefinition, ids::ChannelId};

pub(super) fn channel_endpoints(
    state: &ManagementState,
    channel_id: &ChannelId,
    definition: &ChannelDefinition,
) -> EndpointResponse {
    EndpointResponse {
        native_lark_webhook_url: definition
            .sources
            .iter()
            .any(|s| matches!(s, cmaeg_core::channel::Source::NativeLark(_)))
            .then(|| {
                format!(
                    "{}/v1/native-lark/{}",
                    state.advertised_api_url(),
                    channel_id
                )
            }),
        whatsapp_webhook_url: definition
            .sources
            .iter()
            .any(|source| matches!(source, cmaeg_core::channel::Source::Whatsapp { .. }))
            .then(|| format!("{}/v1/whatsapp/{}", state.advertised_api_url(), channel_id)),
        nyxid_callback_url: format!("{}/v1/callbacks/{}", state.advertised_api_url(), channel_id),
        telegram_webhook_url: definition
            .sources
            .iter()
            .any(|source| matches!(source, cmaeg_core::channel::Source::Telegram { .. }))
            .then(|| format!("{}/v1/telegram/{}", state.advertised_api_url(), channel_id)),
        custom_event_url: format!(
            "{}/v1/channels/{}/events",
            state.advertised_api_url(),
            channel_id
        ),
    }
}
