use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{launch_service_support::store_error, LaunchServiceError, LaunchStore};
use crate::{Caller, Capability};

/// Deployment policy; never part of a retained Launch or public request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LaunchAdmissionPolicy {
    #[default]
    Baseline,
    VisitorAllowance,
}

impl super::LaunchService {
    pub fn with_admission_policy(
        &self,
        admission_policy: LaunchAdmissionPolicy,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            admission_policy,
            ..self.clone()
        })
    }

    pub(super) async fn check_visitor_allowance(
        &self,
        caller: &Caller,
        limit: u32,
    ) -> Result<(), LaunchServiceError> {
        if self.admission_policy == LaunchAdmissionPolicy::Baseline {
            return Ok(());
        }
        let now = crate::seconds_from_rfc3339(&crate::now_rfc3339())
            .ok_or_else(|| LaunchServiceError::Unavailable("Launch clock is unavailable".into()))?;
        refuse_exhausted_visitor(self.launches.as_ref(), caller, limit, now).await
    }
}

/// Return the UTC day and next midnight for an admitted Launch timestamp.
pub fn visitor_launch_day(created_at_seconds: u64) -> (u64, SystemTime) {
    let day = created_at_seconds / 86_400;
    (day, UNIX_EPOCH + Duration::from_secs((day + 1) * 86_400))
}

/// Avoid expensive dependencies after the monotonically increasing count is spent.
/// Admission remains authoritative; a concurrent limit increase can cause one
/// refusal using the previously read policy.
pub(super) async fn refuse_exhausted_visitor(
    launches: &dyn LaunchStore,
    caller: &Caller,
    limit: u32,
    now_seconds: u64,
) -> Result<(), LaunchServiceError> {
    if caller.authorization.allows(Capability::CreateSandbox) {
        return Ok(());
    }
    let (day, reset_at) = visitor_launch_day(now_seconds);
    let used = launches
        .visitor_launches_admitted(&caller.subject, day)
        .await
        .map_err(store_error)?;
    if used >= limit {
        return Err(LaunchServiceError::VisitorQuota { limit, reset_at });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn utc_day_and_reset_follow_midnight() {
        assert_eq!(
            visitor_launch_day(86_399),
            (0, UNIX_EPOCH + Duration::from_secs(86_400))
        );
        assert_eq!(
            visitor_launch_day(86_400),
            (1, UNIX_EPOCH + Duration::from_secs(172_800))
        );
    }
}
