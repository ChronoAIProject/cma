//! Optional exact finality for an owner-admitted whole-runtime retirement.
use crate::{AdapterError, Result, RuntimeAdoptionRequest};

/// Provider identity of the receiver, excluding its address and credentials.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementPod {
    pub uid: String,
    pub container_id: String,
    pub restart_count: i32,
}

impl TryFrom<&crate::agent_sandbox::PodBinding> for RetirementPod {
    type Error = AdapterError;

    fn try_from(binding: &crate::agent_sandbox::PodBinding) -> Result<Self> {
        let Some(container_id) = &binding.runtime_container_id else {
            return Err(AdapterError::Unavailable(
                "retirement receiver has no container identity".into(),
            ));
        };
        let Some(restart_count) = binding.runtime_restart_count else {
            return Err(AdapterError::Unavailable(
                "retirement receiver has no container restart count".into(),
            ));
        };
        if binding.pod_uid.is_empty() || container_id.is_empty() || restart_count < 0 {
            return Err(AdapterError::Rejected(
                "retirement receiver identity is invalid".into(),
            ));
        }
        Ok(Self {
            uid: binding.pod_uid.clone(),
            container_id: container_id.clone(),
            restart_count,
        })
    }
}

#[async_trait::async_trait]
pub trait RuntimeRetirement: Send + Sync {
    /// Repeat one ownership challenge without confirming or renewing it.
    /// A correlated failure for the exact native receiver returns true.
    async fn challenge_retirement_receiver(
        &self,
        request: &RuntimeAdoptionRequest,
        receiver: &crate::receiver_failure::RetirementReceiver,
    ) -> Result<bool>;

    /// Validate the original request/home, then perform ordinary fenced deletion.
    /// A missing control endpoint never proves that the original processes stopped.
    async fn retire_unreachable(
        &self,
        request: &RuntimeAdoptionRequest,
        receiver: &RetirementPod,
    ) -> Result<()>;

    /// Fresh read-only proof that the original runtime, processes and claims are absent.
    /// Call after capturing the observation boundary, even after successful deletion.
    async fn verify_retired(&self, request: &RuntimeAdoptionRequest) -> Result<()>;
}
