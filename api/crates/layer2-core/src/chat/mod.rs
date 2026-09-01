//! Deployment-owned chat configuration and host lifecycle state.

mod checkpoint;
mod checkpoints;
mod host;
mod inactivity;
mod settings;
mod threads;

pub use checkpoint::{
    ChatCheckpointConversation, ChatCheckpointConversationRef, ChatCheckpointRef,
    MAX_CHAT_CONVERSATION_SNAPSHOT_BYTES,
};
pub use checkpoints::{
    ChatCheckpointOutcome, ChatCheckpointSweep, CHAT_CHECKPOINT_INTERVAL_SECONDS,
};
pub use host::{
    ChatHostFailure, ChatHostId, ChatHostListQuery, ChatHostOperatorNote, ChatHostRecord,
    ChatHostStatus, ChatProvisioningPhase, ChatThreadSlot, ChatUnavailableCause,
    ChatWorkspaceAuthority,
};
pub use inactivity::{ChatPauseOutcome, CHAT_PAUSE_REFUSAL_BACKOFF_SECONDS};
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

#[cfg(test)]
#[path = "checkpoint_service_tests.rs"]
mod checkpoint_service_tests;

#[cfg(test)]
#[path = "inactivity_service_tests.rs"]
mod inactivity_service_tests;
