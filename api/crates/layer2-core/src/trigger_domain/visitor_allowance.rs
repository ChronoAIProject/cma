use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{launch_service_support::store_error, LaunchServiceError, LaunchStore};
use crate::{Caller, Capability};

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
