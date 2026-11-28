//! Per-Pod assignment delivered over Kubernetes exec after a native pool claim.
use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{AdapterError, Result};

pub use crate::runtime_versions::RUNTIME_BOOTSTRAP_VERSION;
pub const RUNTIME_BOOTSTRAP_LABEL: &str = "dev.chrono.cma.runtime-bootstrap";
pub const RUNTIME_HOME_LAYOUT_LABEL: &str = "dev.chrono.cma.home-layout";
pub const NATIVE_ASSIGNMENT_DEADLINE: Duration = Duration::from_secs(30);

mod staging_startup;
pub use staging_startup::StagingStartupFailure;

pub const MAX_RUNTIME_BOOTSTRAP_BYTES: usize = 64 * 1024;
pub const RUNTIME_BOOTSTRAP_SOCKET: &str = "/var/lib/cma-runtime/assignment.sock";

/// Pool templates require explicit image support in addition to contract 3.
pub fn admit_pool_image(image: &crate::RuntimeImage) -> Result<()> {
    crate::RuntimeContract::admit(&image.uri, &image.labels)?;
    if image
        .labels
        .get(RUNTIME_BOOTSTRAP_LABEL)
        .map(String::as_str)
        != Some(crate::runtime_versions::RUNTIME_BOOTSTRAP_VERSION_LABEL)
    {
        return Err(AdapterError::Rejected(
            "image does not support runtime pool assignment".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBootstrapRequest {
    pub version: u16,
    pub pod_uid: String,
    pub environment: BTreeMap<String, String>,
    /// Operator-owned qualification, outside the tenant environment map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_startup_failure: Option<StagingStartupFailure>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub staging_startup_confirmed: bool,
}

impl std::fmt::Debug for RuntimeBootstrapRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeBootstrapRequest")
            .field("version", &self.version)
            .field("pod_uid", &self.pod_uid)
            .field("environment", &"<redacted>")
            .field(
                "staging_startup_failure",
                &self.staging_startup_failure.is_some(),
            )
            .finish()
    }
}

impl RuntimeBootstrapRequest {
    pub fn validate(&self) -> Result<()> {
        let value = |name: &str| self.environment.get(name).map(String::as_str).unwrap_or("");
        let generation = value("CHRONO_RUNTIME_GENERATION");
        if self.version != RUNTIME_BOOTSTRAP_VERSION
            || !valid_pod_uid(&self.pod_uid)
            || self
                .staging_startup_failure
                .as_ref()
                .is_some_and(|fault| !fault.valid_for_assignment(value("CHRONO_SANDBOX_ID")))
            || self.staging_startup_confirmed && self.staging_startup_failure.is_none()
            || self.environment.len() > 128
            || self.environment.iter().any(|(name, value)| {
                name.is_empty()
                    || name.len() > 128
                    || name.as_bytes()[0].is_ascii_digit()
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || value.len() > 16 * 1024
                    || value.contains('\0')
            })
            || value("CHRONO_SANDBOX_ID").is_empty()
            || value("CHRONO_SANDBOX_ID").len() > 256
            || value("CHRONO_SANDBOX_ID").chars().any(char::is_whitespace)
            || generation
                .parse::<u64>()
                .ok()
                .is_none_or(|g| g == 0 || g.to_string() != generation)
            || !crate::valid_home_identity(value("CHRONO_RUNTIME_HOME_IDENTITY"))
            || !crate::provider_finality::digest(value("CHRONO_RUNTIME_CONTROL_TOKEN"))
            || self.environment.contains_key("CHRONO_RUNTIME_POD_UID")
            || self
                .environment
                .contains_key("CHRONO_RUNTIME_BOOTSTRAP_SOCKET")
        {
            return Err(invalid());
        }
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() > MAX_RUNTIME_BOOTSTRAP_BYTES {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn request_sha256(&self) -> Result<String> {
        self.validate()?;
        crate::provider_request_digest(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBootstrapReceipt {
    pub version: u16,
    pub pod_uid: String,
    pub request_sha256: String,
}

impl RuntimeBootstrapReceipt {
    pub fn for_request(request: &RuntimeBootstrapRequest) -> Result<Self> {
        Ok(Self {
            version: RUNTIME_BOOTSTRAP_VERSION,
            pod_uid: request.pod_uid.clone(),
            request_sha256: request.request_sha256()?,
        })
    }

    pub fn validate_for(&self, request: &RuntimeBootstrapRequest) -> Result<()> {
        if self != &Self::for_request(request)? {
            return Err(invalid());
        }
        Ok(())
    }
}

pub fn valid_pod_uid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("runtime pool assignment is invalid".into())
}

#[cfg(test)]
#[path = "runtime_bootstrap_tests.rs"]
mod tests;
