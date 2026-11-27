//! Hosted sandbox implementation shared by agent providers.

mod acp;
mod acp_context_refresh;
mod acp_correlation;
mod acp_credit_failure;
mod acp_handshake;
mod acp_inbound;
mod acp_observations;
mod acp_open_completion;
mod acp_open_reply_loss;
mod acp_permission;
mod acp_provider_failure;
mod acp_session_identity;
mod acp_settings;
mod acp_turn;
mod adapter;
mod agent_sandbox_backend;
mod backend;
mod retained_startup_policy;
pub use retained_startup_policy::HostedRetainedStartupPolicy;
mod capacity_recovery;
pub use capacity_recovery::CapacityRecoveryNativeTransport;
mod channel_tools;
mod checkpoint_store;
mod checkpoint_transport;
mod context_goal;
mod control_correlation;
mod credit_error_json;
mod deployment;
mod deployment_request;
mod home;
#[cfg(test)]
mod home_tests;
mod hosted_control_transport;
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
#[cfg(test)]
mod released_selection_tests;
mod reply;
mod retained_json;
mod runtime_decode;
mod sign_in_retention;
mod startup_timing;
mod subscriptions;
mod tool_confirmation;
mod update_relay;

pub use acp_open_completion::HostedOpenCompletionProtocol;
pub use acp_open_reply_loss::HostedOpenReplyLoss;
pub use adapter::HostedAdapter;
pub use agent_sandbox_backend::{
    AgentSandboxBackend, NativeStatusInspection, RuntimeBackends, RuntimeRoutingStore,
};
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
pub(crate) use deployment_request::JOURNAL_RECOVERY;
pub use hosted_control_transport::HostedControlTransport;
pub use managed_context::HostedContextRefresh;
pub use memory::{InMemoryHostedBackend, InMemoryRuntimeSnapshot};
pub(crate) use nonexecuting::IdleBootstrapProof;
#[doc(hidden)]
pub use nonexecuting::NonexecutingRuntimeProof;
pub use provider_backend::ProviderHostedBackend;
pub use provider_tools::{
    HostedProviderToolBinding, HostedProviderToolSource, HostedToolOpenReceipt,
};
pub use provider_transport::{
    LegacyReceiverImage, NativeRuntimeTransport, ProviderHostedControlTransport,
};
pub use subscriptions::{HostedSubscriptionCredential, HostedSubscriptionSource};

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

#[cfg(test)]
mod process_ingress_test_support;

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

/// Derive the same permission identity used by the ACP bridge and durable replies.
/// Runtime observations use this contract rather than inventing an approval ID.
pub fn observed_permission_id(
    request: &crate::RequestPermissionRequest,
    call: &serde_json::Value,
) -> Option<String> {
    if !acp_correlation::valid_call(call)
        || permission_state::validate(&request.options).is_err()
        || request.options.is_empty()
        || request.options.iter().any(|option| {
            !matches!(
                option.kind,
                crate::PermissionOptionKind::AllowOnce
                    | crate::PermissionOptionKind::AllowAlways
                    | crate::PermissionOptionKind::RejectOnce
                    | crate::PermissionOptionKind::RejectAlways
            )
        })
    {
        return None;
    }
    Some(acp_inbound::permission_id(
        &acp_inbound::permission_locator(request.session_id.0.as_ref(), call, &request.options),
    ))
}

/// Trusted provider-rendered maintenance command; never user-supplied exec.
/// The lifecycle adapter supplies authority for inspection or admitted recovery.
/// Configuration belongs on stdin, not in Kubernetes command arguments.
/// Fields are crate-private to keep command construction inside the adapter.
#[doc(hidden)]
pub struct RuntimeRecoveryCommand {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) input: Vec<u8>,
}
