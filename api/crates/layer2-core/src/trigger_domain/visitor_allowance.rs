use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Return the UTC day and next midnight for an admitted Launch timestamp.
pub fn visitor_launch_day(created_at_seconds: u64) -> (u64, SystemTime) {
    let day = created_at_seconds / 86_400;
    (day, UNIX_EPOCH + Duration::from_secs((day + 1) * 86_400))
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
