//! Read back the effective startup policy from an identified Supervisor process.

use crate::{
    wire::ResponseFrame, AdapterError, Result, RuntimeAdoptionRequest, RuntimeHandle,
    RuntimeProvisionRequest, RuntimeStatus, RuntimeStatusRequest,
};
use serde::{Deserialize, Serialize};

pub const RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE: &str =
    "application/vnd.cma.runtime-startup-policy.v1+json";
pub(crate) const MAX_RUNTIME_STARTUP_POLICY_STATUS_BYTES: usize = 32_768;

/// Provider evidence to freeze with a retained transition's domain admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedStartupPolicyPredecessor {
    pub uid: String,
    pub resource_version: String,
    pub request_sha256: String,
    pub home_identity: String,
    pub image: String,
    pub generation: u64,
    pub journal_recovery: bool,
}

/// Exact intent frozen by the retained-home admission transaction.
pub struct RetainedStartupPolicyUpgrade<'a> {
    pub operation_id: &'a str,
    pub predecessor: &'a RetainedStartupPolicyPredecessor,
    pub target_image: &'a str,
    pub successor_generation: u64,
    pub journal_recovery: bool,
}

/// Exact running image and Supervisor state required by a retained transition.
pub struct RetainedStartupPolicyReadback<'a> {
    pub runtime: &'a RuntimeStatusRequest,
    pub boot_id: &'a str,
    pub image: &'a str,
    pub config_digest: &'a str,
    pub journal_recovery: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStartupPolicyObservation {
    pub runtime: RuntimeStatus,
    pub legacy_journal_recovery_enabled: bool,
}

impl RuntimeStartupPolicyObservation {
    /// Older Supervisors answer with ordinary status. Absence cannot prove that recovery is off.
    pub fn decode(bytes: &[u8], request: &RuntimeStatusRequest) -> Result<Option<Self>> {
        if bytes.len() <= MAX_RUNTIME_STARTUP_POLICY_STATUS_BYTES {
            if let Ok(observation) = serde_json::from_slice::<Self>(bytes) {
                observation.runtime.validate_for(request)?;
                return Ok(Some(observation));
            }
            if let Ok(ResponseFrame::RuntimeStatus(status)) = serde_json::from_slice(bytes) {
                status.validate_for(request)?;
                return Ok(None);
            }
        }
        Err(AdapterError::Rejected(
            "runtime startup policy observation is invalid".into(),
        ))
    }
}

pub(crate) fn running_image_matches(running_image: &str, manifest: &str, config: &str) -> bool {
    let observed_manifest = running_image
        .strip_prefix("docker-pullable://")
        .unwrap_or(running_image);
    let observed_config = running_image
        .strip_prefix("containerd://")
        .or_else(|| running_image.strip_prefix("docker://"))
        .unwrap_or(running_image);
    observed_manifest == manifest || observed_config == config
}

/// Inspect and replace one retained runtime under its admitted startup policy.
#[async_trait::async_trait]
pub trait RetainedStartupPolicyControl: Send + Sync {
    async fn inspect_retained_startup_policy(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<RetainedStartupPolicyPredecessor>;

    async fn upgrade_retained_runtime_policy(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
        admission: &RetainedStartupPolicyUpgrade<'_>,
    ) -> Result<RuntimeHandle>;

    async fn verify_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> Result<RuntimeStartupPolicyObservation>;
}
