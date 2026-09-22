//! Anonymous provider discovery, independent of A2A message support.
use crate::{Client, Result, http::Request};
use serde::Deserialize;

/// Public provider discovery document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderAgentCard {
    /// Human-readable provider name.
    pub name: String,
    /// Implementation version.
    pub version: String,
    /// Configured public origin.
    pub url: String,
    /// Advertised, independently verified capabilities.
    pub capabilities: ProviderCapabilities,
    /// Binding and Responses routes.
    pub provider_contract: ProviderContract,
    /// Accepted credential schemes.
    pub authentication: ProviderAuthentication,
}
/// Capability flags; an advertised level does not imply intermediate capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderCapabilities {
    /// Streaming Responses.
    pub streaming: bool,
    /// Informational provider activity.
    pub activity_items: bool,
    /// Client-executed tools.
    pub client_tools: bool,
    /// Explicit response cancellation controls.
    pub cancel: bool,
    /// Rich input content.
    pub rich_input: bool,
}
/// Route templates for provider operations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderContract {
    /// Binding management route.
    pub binding: String,
    /// Conversation management route.
    pub conversation: String,
    /// Streaming response creation route.
    pub responses: String,
}
/// Advertised credential schemes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderAuthentication {
    /// Accepted credential scheme names.
    pub schemes: Vec<String>,
}
impl Client {
    /// Read public provider discovery without sending the configured credential.
    pub async fn read_agent_card(&self) -> Result<ProviderAgentCard> {
        let mut request = Request::get(&[".well-known", "agent-card.json"]);
        request.root = true;
        request.anonymous = true;
        self.request(request).await
    }
}
