//! Optional exact finality for an owner-admitted whole-runtime retirement.
use crate::{Result, RuntimeAdoptionRequest};

#[async_trait::async_trait]
pub trait RuntimeRetirement: Send + Sync {
    /// Validate the original request/home, then perform ordinary fenced deletion.
    /// A missing control endpoint never proves that the original processes stopped.
    async fn retire_unreachable(&self, request: &RuntimeAdoptionRequest) -> Result<()>;

    /// Fresh read-only proof that the original runtime, processes and claims are absent.
    /// Call after capturing the observation boundary, even after successful deletion.
    async fn verify_retired(&self, request: &RuntimeAdoptionRequest) -> Result<()>;
}
