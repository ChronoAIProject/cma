use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// NyxID emits chrono RFC 3339 values, including fractions and numeric offsets.
/// The core's internal twenty-character UTC writer is a separate contract.
/// Returns whole Unix seconds without changing the retained provider value.
pub fn provider_timestamp(value: &str) -> Option<i64> {
    if value.len() > 64 {
        return None;
    }
    let seconds = OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .unix_timestamp();
    (seconds >= 0).then_some(seconds)
}
