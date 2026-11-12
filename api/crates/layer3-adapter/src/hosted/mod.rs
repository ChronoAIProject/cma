//! Hosted sandbox implementation shared by agent providers.

mod acp;
mod acp_context_refresh;
mod acp_credit_failure;
mod acp_handshake;
mod acp_inbound;
mod acp_observations;
mod acp_open_completion;
mod acp_permission;
mod acp_settings;
mod acp_turn;
mod adapter;
mod agent_sandbox_backend;
mod backend;
mod capacity_recovery;
pub use capacity_recovery::CapacityRecoveryNativeTransport;
mod channel_tools;
mod checkpoint_store;
mod checkpoint_transport;
mod context_goal;
mod control_correlation;
mod credit_error_json;
mod deployment;
mod home;
#[cfg(test)]
mod home_tests;
#[cfg(test)]
mod indexed_ledger_hold_tests;
mod inspection_timing;
#[cfg(test)]
mod ledger_hold_tests;
mod managed_context;
mod memory;
mod nonexecuting;
mod observation_retention;
mod output_json_budget;
mod permission_state;
mod provider_backend;
mod provider_endpoint;
mod provider_http;
mod provider_tools;
mod provider_transport;
mod reply;
mod retained_json;
mod runtime_decode;
mod sign_in_retention;
mod startup_timing;
mod subscriptions;
mod tool_confirmation;
mod update_relay;

pub use acp_open_completion::HostedOpenCompletionProtocol;
pub use adapter::HostedAdapter;
pub use agent_sandbox_backend::{
    AgentSandboxBackend, NativeStatusInspection, RuntimeBackends, RuntimeRoutingStore,
};
#[doc(hidden)]
pub use backend::RuntimeRecoveryCommand;
pub use backend::{HostedAgentOpenSettings, HostedAgentRuntime, HostedBackend};
pub use channel_tools::{HostedChannelToolBinding, HostedChannelToolSource};
pub use checkpoint_store::{
    CheckpointArchiveDownload, CheckpointArchiveReceipt, CheckpointArchiveStore,
    CheckpointArchiveStoreFactory, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
pub(crate) use deployment::{
    image_sha256, provider_request_fingerprint, provider_request_identity, scoped_model_base_url,
    validate_runtime_image,
};
pub use deployment::{
    HostedDeployment, MODEL_PROXY_BASE_URL, NYXID_PROXY_BASE_URL, PLACEHOLDER_CREDENTIAL,
};
pub use managed_context::HostedContextRefresh;
pub use memory::{InMemoryHostedBackend, InMemoryRuntimeSnapshot};
pub(crate) use nonexecuting::IdleBootstrapProof;
#[doc(hidden)]
pub use nonexecuting::NonexecutingRuntimeProof;
pub use provider_backend::{HostedControlTransport, ProviderHostedBackend};
pub use provider_tools::{
    HostedProviderToolBinding, HostedProviderToolSource, HostedToolOpenReceipt,
};
pub use provider_transport::{NativeRuntimeTransport, ProviderHostedControlTransport};
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

mod access_tools;
pub use access_tools::{HostedAccessToolBinding, HostedAccessToolSource};
