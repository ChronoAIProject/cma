//! Deployment-owned chat configuration and host lifecycle state.

mod checkpoint;
mod host;
mod settings;
mod threads;

pub use checkpoint::{ChatCheckpointConversationRef, ChatCheckpointRef};
pub use host::{
    ChatHostFailure, ChatHostId, ChatHostListQuery, ChatHostRecord, ChatHostStatus,
    ChatProvisioningPhase, ChatThreadSlot, ChatUnavailableCause,
};
pub use settings::{
    ChatAgentSettings, ChatHostSettings, ChatInactivity, ChatSettings, ChatSettingsSnapshot,
    ChatSettingsViolation, ChatSetup, ChatWorkspaceSource,
};

#[cfg(test)]
#[path = "host_tests.rs"]
mod host_tests;

#[cfg(test)]
#[path = "host_service_tests.rs"]
mod host_service_tests;
