//! Metadata projections of the pinned identity provider's channel resources.
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bots {
    pub bots: Vec<Bot>,
    pub total: u64,
}
#[derive(Deserialize)]
pub(crate) struct Bot {
    pub id: String,
    pub user_id: String,
    pub platform: String,
    pub is_active: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Routes {
    pub conversations: Vec<Route>,
    pub total: u64,
}
#[derive(Deserialize)]
pub(crate) struct Route {
    pub id: String,
    pub channel_bot_id: Option<String>,
    /// Every platform stays visible to callers checking for conflicting key use.
    pub platform: String,
    pub agent_api_key_id: String,
    pub is_active: bool,
}
