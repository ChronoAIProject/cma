//! Deployment-owned chat configuration and host lifecycle state.

mod settings;

pub use settings::{
    ChatAgentSettings, ChatHostSettings, ChatInactivity, ChatSettings, ChatSettingsSnapshot,
    ChatSettingsViolation, ChatSetup, ChatWorkspaceSource,
};
