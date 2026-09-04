//! Hosted sandbox provider lifecycle client.

mod client;
mod model;
mod reclaimer;

pub use client::SandboxClient;
pub use model::{
    Pagination, ProviderEndpoint, ProviderError, ProviderImage, ProviderSandbox,
    ProviderSandboxRequest, ProviderSandboxStatus,
};
pub use reclaimer::BatchSandboxReclaimer;

pub(crate) use client::ProviderDeleteTarget;

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
