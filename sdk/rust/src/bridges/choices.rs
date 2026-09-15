//! Safe existing provider object choices. These objects grant no execution rights.
use super::BridgePlatform;
use serde::Deserialize;

/// Existing bot selectable by the current owner/admin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeBotChoice {
    /// Existing bot ID.
    pub id: String,
    /// Existing bot platform.
    pub platform: BridgePlatform,
    /// Current provider-observed availability.
    pub active: bool,
}
/// Existing key metadata without secret material.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeKeyChoice {
    /// Existing key ID.
    pub id: String,
    /// Current provider-observed availability.
    pub active: bool,
    /// Whether a provider callback is already configured.
    pub callback_configured: bool,
}
/// Existing route selection; the route ID is distinct from the actual chat ID.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeRouteChoice {
    /// Existing route ID.
    pub id: String,
    /// Selected bot, when provider metadata supplies one.
    pub bot_id: Option<String>,
    /// Existing associated key ID.
    pub key_id: String,
    /// Provider platform string, including unsupported choices that cannot be selected.
    pub platform: String,
    /// Current provider-observed availability.
    pub active: bool,
}
/// Current safe metadata for explicitly selecting existing objects.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeConnectorChoices {
    /// Visible existing bots.
    pub bots: Vec<BridgeBotChoice>,
    /// Visible existing key metadata.
    pub keys: Vec<BridgeKeyChoice>,
    /// Visible existing routes.
    pub routes: Vec<BridgeRouteChoice>,
}
