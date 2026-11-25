//! Release resolution stays outside startup and Sandbox request paths.

mod controller;
pub(crate) mod operator;
mod registry;
pub(crate) use controller::RuntimePointerController;
pub(crate) use registry::valid_digest;
pub(crate) use registry::{RegistryRuntimePointers, RuntimePointerConfig};

use async_trait::async_trait;
use layer2_core::released_runtime::ReleasedLane;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PointerObservation {
    Verified(String),
    Refused { image: String, reason: &'static str },
}

#[derive(Debug, thiserror::Error)]
#[error("released runtime pointer read is unavailable")]
pub(crate) struct PointerUnavailable;

/// Each observation resolves a tag once and verifies the exact immutable image.
#[async_trait]
pub(crate) trait RuntimePointerSource: Send + Sync {
    async fn observe(
        &self,
        lane: ReleasedLane,
    ) -> Result<Option<PointerObservation>, PointerUnavailable>;
}
