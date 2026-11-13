//! Ordinary work authority is independent of an operation's execution timeout.

use std::time::{Duration, SystemTime};

use super::SandboxStoreError;

pub const WORK_LEASE_SECONDS: u64 = 15;
pub const WORK_RENEW_SECONDS: u64 = 3;

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
