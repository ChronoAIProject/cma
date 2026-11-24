//! Read back the effective startup policy from an identified Supervisor process.

use crate::{wire::ResponseFrame, AdapterError, Result, RuntimeStatus, RuntimeStatusRequest};
use serde::{Deserialize, Serialize};

pub const RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE: &str =
    "application/vnd.cma.runtime-startup-policy.v1+json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStartupPolicyObservation {
    pub runtime: RuntimeStatus,
    pub legacy_journal_recovery_enabled: bool,
}

impl RuntimeStartupPolicyObservation {
    /// Older Supervisors answer with ordinary status. Absence cannot prove that recovery is off.
    pub fn decode(bytes: &[u8], request: &RuntimeStatusRequest) -> Result<Option<Self>> {
        if bytes.len() <= 32_768 {
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
