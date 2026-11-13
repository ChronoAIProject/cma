//! Ordinary work authority is independent of an operation's execution timeout.

use std::time::{Duration, SystemTime};

use super::SandboxStoreError;

/// A retained reader/writer floor, independent of deployment feature switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkAuthorityProtocol {
    #[serde(rename = "renewed_v1")]
    RenewedV1,
}

pub const WORK_LEASE_SECONDS: u64 = 15;
pub const WORK_RENEW_SECONDS: u64 = 3;

/// Remaining authority measured by the store at the renewal write. Callers
/// start this duration at their monotonic request start, never at response time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkLeaseGrant {
    pub expires_at: String,
    pub valid_for: Duration,
}

impl WorkLeaseGrant {
    pub fn new(expires_at: String, remaining_millis: i64) -> Result<Self, SandboxStoreError> {
        if crate::seconds_from_rfc3339(&expires_at).is_none()
            || !(1..=WORK_LEASE_SECONDS as i64 * 1000).contains(&remaining_millis)
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(Self {
            expires_at,
            valid_for: Duration::from_millis(remaining_millis as u64),
        })
    }

    pub(super) fn at(expires_at: String, now: &str) -> Result<Self, SandboxStoreError> {
        let remaining = crate::seconds_from_rfc3339(&expires_at)
            .zip(crate::seconds_from_rfc3339(now))
            .and_then(|(until, now)| until.checked_sub(now))
            .and_then(|seconds| seconds.checked_mul(1000))
            .and_then(|millis| i64::try_from(millis).ok())
            .ok_or(SandboxStoreError::Conflict)?;
        Self::new(expires_at, remaining)
    }
}

pub(super) fn renewal_deadline(
    now: &str,
    requested_until: &str,
) -> Result<String, SandboxStoreError> {
    if crate::seconds_from_rfc3339(requested_until).is_none() || requested_until <= now {
        return Err(SandboxStoreError::Conflict);
    }
    let seconds = crate::seconds_from_rfc3339(now)
        .and_then(|seconds| seconds.checked_add(WORK_LEASE_SECONDS))
        .ok_or(SandboxStoreError::Conflict)?;
    let until = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .ok_or(SandboxStoreError::Conflict)?;
    Ok(crate::rfc3339_at(until).min(requested_until.to_owned()))
}
