use layer3_adapter::agent_sandbox::{NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup};
use layer3_adapter::HomeVolumeRequest;
use serde::{Deserialize, Serialize};

use super::SandboxModelError;

/// Every transition is stored before the next provider effect. No native claim
/// can fall back to preparing an ordinary deterministic home after a retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxNativePool {
    pub request: NativePoolClaimRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<NativePoolClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<NativePoolCleanup>,
    #[serde(default)]
    pub cleaned: bool,
}

impl SandboxNativePool {
    pub fn validate_for(&self, home: &HomeVolumeRequest) -> Result<(), SandboxModelError> {
        let invalid = || SandboxModelError::invalid_value("sandbox.home.native_pool");
        self.request.validate().map_err(|_| invalid())?;
        if &self.request.home != home {
            return Err(invalid());
        }
        if let Some(claim) = &self.claim {
            if claim.request != self.request
                || claim.name.is_empty()
                || claim.uid.is_empty()
                || claim.resource_version.is_empty()
                || claim.sandbox_name.as_ref().is_some_and(String::is_empty)
            {
                return Err(invalid());
            }
        }
        if let Some(cleanup) = &self.cleanup {
            cleanup.validate().map_err(|_| invalid())?;
            if cleanup.request != self.request
                || self.claim.as_ref().is_some_and(|claim| {
                    cleanup.claim.as_ref().is_none_or(|c| {
                        c.uid != claim.uid
                            || c.name != claim.name
                            || (claim.sandbox_name.is_some()
                                && c.sandbox_name != claim.sandbox_name)
                    })
                })
            {
                return Err(invalid());
            }
        } else if self.cleaned {
            return Err(invalid());
        }
        Ok(())
    }
}
