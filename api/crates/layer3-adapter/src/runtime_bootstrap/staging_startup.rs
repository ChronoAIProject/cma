use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MAX_WINDOW: Duration = Duration::from_secs(45 * 60);

/// A time-bounded startup failure supplied by the staging assignment authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingStartupFailure {
    sandbox_id: String,
    expires_unix_secs: u64,
    incarnation: Option<String>,
}

impl StagingStartupFailure {
    pub fn from_setting(environment: Option<&str>, value: &str, now: SystemTime) -> Option<Self> {
        if environment != Some("staging") {
            return None;
        }
        let (sandbox_id, expires) = value.split_once(':')?;
        let fault = Self {
            sandbox_id: sandbox_id.into(),
            expires_unix_secs: expires.parse().ok()?,
            incarnation: None,
        };
        let expiry = UNIX_EPOCH.checked_add(Duration::from_secs(fault.expires_unix_secs))?;
        (fault.valid_for(sandbox_id) && expiry <= now.checked_add(MAX_WINDOW)?).then_some(fault)
    }

    pub fn valid_for(&self, sandbox_id: &str) -> bool {
        self.sandbox_id == sandbox_id
            && sandbox_id.strip_prefix("sbx_").is_some_and(|id| {
                id.len() == 32
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            })
    }

    pub fn armed_at(&self, now: SystemTime) -> bool {
        let expiry = UNIX_EPOCH.checked_add(Duration::from_secs(self.expires_unix_secs));
        expiry.is_some_and(|expiry| {
            expiry
                .duration_since(now)
                .is_ok_and(|left| !left.is_zero() && left <= MAX_WINDOW)
        })
    }

    pub fn bind_incarnation(mut self, container_id: &str) -> Option<Self> {
        if container_id.is_empty() {
            return None;
        }
        self.incarnation = Some(crate::provider_request_digest(&container_id).ok()?);
        Some(self)
    }

    pub(super) fn valid_for_assignment(&self, sandbox_id: &str) -> bool {
        self.valid_for(sandbox_id)
            && self
                .incarnation
                .as_deref()
                .is_some_and(crate::provider_finality::digest)
    }

    pub(crate) fn targets_assignment(
        &self,
        sandbox_id: &str,
        restart_count: Option<i32>,
        now: SystemTime,
    ) -> bool {
        // Once the third incarnation fails, offer normal startup on the
        // breaker's first allowed probe without restarting Agent Manager.
        restart_count.is_some_and(|count| (0..3).contains(&count))
            && self.valid_for(sandbox_id)
            && self.armed_at(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_failure_targets_only_three_incarnations_of_one_sandbox() {
        let sandbox_id = "sbx_00000000000000000000000000000001";
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let fault =
            StagingStartupFailure::from_setting(Some("staging"), &format!("{sandbox_id}:200"), now)
                .unwrap();
        for count in 0..3 {
            assert!(fault.targets_assignment(sandbox_id, Some(count), now));
        }
        for count in [None, Some(-1), Some(3), Some(4)] {
            assert!(!fault.targets_assignment(sandbox_id, count, now));
        }
        assert!(!fault.targets_assignment("sbx_00000000000000000000000000000002", Some(0), now));
        assert!(!fault.targets_assignment(sandbox_id, Some(0), now + Duration::from_secs(100)));
    }
}
