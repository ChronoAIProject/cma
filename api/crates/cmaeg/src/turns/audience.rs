//! What the agent behind a binding can do with an attachment, which decides
//! the hints a turn's attachment block carries.
use crate::MediaToolProviders;
use cmaeg_core::{channel::ConsumerView, profile::ActionMode};

/// What the agent behind a binding can do with an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audience {
    /// The binding installs the gateway MCP tools.
    pub tools: bool,
    /// The agent may also save originals into its workspace.
    pub downloads: bool,
}

impl Audience {
    /// Existing-agent bindings never receive gateway tools, and a provider
    /// outside the configured set does not install them.
    pub fn of(consumer: &ConsumerView<'_>, providers: &MediaToolProviders) -> Self {
        let tools =
            consumer.provider.target.is_profile() && providers.contains(&consumer.provider.slug);
        Self {
            tools,
            downloads: tools && consumer.profile.permission.action_mode == ActionMode::Autonomous,
        }
    }
}
