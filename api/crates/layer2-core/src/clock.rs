//! Wall-clock timestamps in the one format the model uses.
//!
//! Kept separate so the service module stays about sessions, and so the date
//! arithmetic is testable on its own. A dedicated date crate would be a
//! dependency earned by one format string.

/// The current time as RFC 3339 in UTC.
pub(crate) fn now_rfc3339() -> String {
    rfc3339_at(std::time::SystemTime::now())
}

/// A given instant as RFC 3339 in UTC.
///
/// Public so an interface can render a moment it computed itself — a deadline,
/// an expiry — in the one format everything else in this model uses. A second
/// format reaching a client would be this service disagreeing with itself
/// about what a time looks like.
pub fn rfc3339_at(time: std::time::SystemTime) -> String {
    let now = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Minimal RFC 3339 in UTC. Avoids a date dependency for a single format.
    let days = now / 86_400;
    let secs = now % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Days since the Unix epoch to a civil date, per Howard Hinnant's algorithm.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_as_the_unix_epoch_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn a_known_day_count_formats_correctly() {
        // 2026-08-12 is 20677 days after the epoch.
        assert_eq!(civil_from_days(20_677), (2026, 8, 12));
    }

    #[test]
    fn a_leap_day_round_trips() {
        // 2024-02-29 is 19782 days after the epoch; leap handling is the most
        // likely thing to be wrong in hand-rolled date arithmetic.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let ts = now_rfc3339();
        assert!(ts.ends_with('Z'), "got {ts}");
        assert_eq!(ts.len(), 20, "got {ts}");
    }
}
