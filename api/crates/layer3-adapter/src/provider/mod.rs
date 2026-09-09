//! Hosted sandbox provider lifecycle client.

mod client;
mod home_volumes;
mod metadata;
mod model;
mod reclaimer;
mod volumes;

pub use client::SandboxClient;
pub use home_volumes::PersistentHomeVolumes;
pub use model::{
    Pagination, ProviderEndpoint, ProviderError, ProviderImage, ProviderSandbox,
    ProviderSandboxRequest, ProviderSandboxStatus,
};
pub use reclaimer::BatchSandboxReclaimer;
pub use volumes::{ProviderPersistentVolumeClaim, ProviderVolume};

pub(crate) use client::ProviderDeleteTarget;

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
mod reclaimer_tests;
