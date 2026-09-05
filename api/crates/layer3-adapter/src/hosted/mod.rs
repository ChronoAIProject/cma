//! Hosted sandbox implementation shared by agent providers.

mod adapter;
mod backend;
mod deployment;
mod memory;
mod provider_backend;
mod reply;

pub use adapter::HostedAdapter;
pub use backend::{HostedAgentRuntime, HostedBackend};
pub use deployment::{
    HostedDeployment, MODEL_PROXY_BASE_URL, NYXID_PROXY_BASE_URL, PLACEHOLDER_CREDENTIAL,
};
pub use memory::{InMemoryHostedBackend, InMemoryRuntimeSnapshot};
pub use provider_backend::{HostedControlTransport, ProviderHostedBackend};

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
