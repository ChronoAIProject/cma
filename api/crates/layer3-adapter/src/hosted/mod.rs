//! Hosted sandbox implementation shared by agent providers.

mod acp;
mod acp_handshake;
mod acp_inbound;
mod acp_turn;
mod adapter;
mod agent_sandbox_backend;
mod backend;
mod channel_tools;
mod checkpoint_store;
mod checkpoint_transport;
mod control_correlation;
mod deployment;
mod home;
#[cfg(test)]
mod home_tests;
mod memory;
mod provider_backend;
mod provider_endpoint;
mod provider_http;
mod provider_tools;
mod provider_transport;
mod reply;
mod subscriptions;
mod update_relay;

pub use adapter::HostedAdapter;
pub use agent_sandbox_backend::{AgentSandboxBackend, RuntimeBackends, RuntimeRoutingStore};
pub use backend::{HostedAgentOpenSettings, HostedAgentRuntime, HostedBackend};
pub use channel_tools::{HostedChannelToolBinding, HostedChannelToolSource};
pub use checkpoint_store::{
    CheckpointArchiveDownload, CheckpointArchiveReceipt, CheckpointArchiveStore,
    CheckpointArchiveStoreFactory, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
pub(crate) use deployment::{provider_request_fingerprint, provider_request_identity};
pub use deployment::{
    HostedDeployment, MODEL_PROXY_BASE_URL, NYXID_PROXY_BASE_URL, PLACEHOLDER_CREDENTIAL,
};
pub use memory::{InMemoryHostedBackend, InMemoryRuntimeSnapshot};
pub use provider_backend::{HostedControlTransport, ProviderHostedBackend};
pub use provider_tools::{
    HostedProviderToolBinding, HostedProviderToolSource, HostedToolOpenReceipt,
};
pub use provider_transport::ProviderHostedControlTransport;
pub use subscriptions::{HostedSubscriptionCredential, HostedSubscriptionSource};

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provision_tests.rs"]
mod provision_tests;

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod receipt_tests;

#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod cleanup_tests;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
