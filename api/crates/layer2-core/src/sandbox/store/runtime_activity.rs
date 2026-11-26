//! Bounded content-free activity observations; observation never admits maintenance.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::SandboxStoreError;
use crate::SandboxId;

mod proof;
mod vector;
pub use vector::{RuntimeActivityCounter, RuntimeActivityRecord, RuntimeActivityVector};

pub const RUNTIME_ACTIVITY_QUIET_MS: i64 = 900_000;
pub const RUNTIME_ACTIVITY_MAX_AGE_MS: i64 = 30_000;
pub const RUNTIME_ACTIVITY_MAX_ROWS: usize = 512;

/// An implementation contract, not a statement that a production source has
/// been qualified. Admission must compare this entire context with configured
/// trusted authority; proof input cannot select or approve its own producer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivitySourceContext {
    pub source_id: String,
    pub adapter_version: String,
    pub policy_id: String,
    pub producer_id: String,
    pub database_id: String,
    pub api_image: String,
    pub source_revision: String,
    pub process_incarnation: String,
    pub serving_processes: u32,
    pub replica_mode: bool,
    pub clock_uncertainty_ms: u32,
    pub coverage: RuntimeActivityCoverage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeActivityCoverage {
    Unqualified,
    /// The trusted source policy must qualify all activity writers, membership
    /// fences, retention/reset rules and pre-effect desktop admission tracking.
    MonotonicWritersV1,
    /// Test-only memory fixtures cannot qualify a Mongo production observation.
    MemoryFixtureV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivityDesktop {
    pub process_incarnation: String,
    pub tracking_incarnation: String,
    pub activity_epoch: u64,
    pub opening_or_open: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivityWatermark {
    pub seconds: u32,
    pub increment: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivitySnapshot {
    pub source: RuntimeActivitySourceContext,
    pub capture_started_at_ms: i64,
    pub capture_completed_at_ms: i64,
    pub watermark: RuntimeActivityWatermark,
    pub storage_incarnation: String,
    pub desktop: RuntimeActivityDesktop,
    pub vector: RuntimeActivityVector,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivityProof {
    pub schema_version: u32,
    pub initial: RuntimeActivitySnapshot,
    pub final_observation: RuntimeActivitySnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeActivityUnknownReason {
    UnqualifiedSource,
    UnsupportedContext,
    MissingWatermark,
    MalformedAuthority,
    IncompleteCoverage,
    CardinalityExceeded,
    SourceChanged,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeActivityObservation {
    Snapshot(Box<RuntimeActivitySnapshot>),
    Unknown(RuntimeActivityUnknownReason),
}

#[async_trait]
pub trait SandboxRuntimeActivityObservationStore: Send + Sync {
    async fn observe_runtime_activity(
        &self,
        sandbox_id: &SandboxId,
        context: &RuntimeActivitySourceContext,
    ) -> Result<RuntimeActivityObservation, SandboxStoreError>;
}

pub(super) fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("runtime activity coverage is incomplete or changed")
}

#[cfg(test)]
mod tests;
