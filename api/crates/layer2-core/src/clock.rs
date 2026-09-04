//! Wall-clock timestamps in the one format the model uses.
//!
//! Kept separate so service modules stay focused, and so the date
//! arithmetic is testable on its own. A dedicated date crate would be a
//! dependency earned by one format string.

/// The current time as RFC 3339 in UTC.
pub fn now_rfc3339() -> String {
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

/// Civil date to days since the Unix epoch, the inverse of [`civil_from_days`].
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(m) + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An RFC 3339 stamp this service wrote, as seconds since the Unix epoch.
///
/// Strictly accepts the twenty-character `YYYY-MM-DDTHH:MM:SSZ` shape emitted
/// by [`rfc3339_at`], and nothing else. That writer truncates to whole seconds,
/// so this clock's resolution is one second. It also maps a pre-epoch
/// [`std::time::SystemTime`] to the epoch; callers using a parsed value as an
/// interval bound must therefore reject zero rather than turn one bad clock
/// read into a charge stretching back to 1970.
pub fn seconds_from_rfc3339(stamp: &str) -> Option<u64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }

    fn number(bytes: &[u8]) -> Option<u32> {
        bytes.iter().try_fold(0_u32, |value, digit| {
            digit
                .is_ascii_digit()
                .then(|| value * 10 + u32::from(*digit - b'0'))
        })
    }

    let year = i64::from(number(&bytes[0..4])?);
    let month = number(&bytes[5..7])?;
    let day = number(&bytes[8..10])?;
    let hour = number(&bytes[11..13])?;
    let minute = number(&bytes[14..16])?;
    let second = number(&bytes[17..19])?;
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    if days < 0 || civil_from_days(days) != (year, month, day) {
        return None;
    }
    let seconds = u64::try_from(days).ok()?.checked_mul(86_400)?;
    seconds
        .checked_add(u64::from(hour) * 3_600)?
        .checked_add(u64::from(minute) * 60)?
        .checked_add(u64::from(second))
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

    #[test]
    fn civil_dates_are_the_inverse_of_day_counts() {
        for days in [0, 1, 365, 19_782, 20_677, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }

    #[test]
    fn service_timestamps_round_trip_to_the_same_second() {
        for seconds in [0, 1, 86_399, 86_400, 1_709_164_800, 4_102_444_799] {
            let instant = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
            assert_eq!(seconds_from_rfc3339(&rfc3339_at(instant)), Some(seconds));
        }
    }

    #[test]
    fn the_epoch_is_a_value_even_though_a_ledger_must_not_use_it_as_a_bound() {
        assert_eq!(seconds_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn timestamp_parsing_refuses_every_shape_the_writer_does_not_emit() {
        for stamp in [
            "",
            "2026-08-23T01:02:03+00:00",
            "2026-08-23T01:02:03.1Z",
            "2026-08-23T01:02:03ZZ",
            "2026-02-30T01:02:03Z",
            "2026-13-23T01:02:03Z",
            "2026-08-23T24:02:03Z",
            "2026-08-23T01:60:03Z",
            "2026-08-23T01:02:60Z",
        ] {
            assert_eq!(seconds_from_rfc3339(stamp), None, "accepted {stamp}");
        }
    }
}
