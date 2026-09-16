use reqwest::header::{HeaderMap, RETRY_AFTER};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Retry-After is pacing metadata, never proof that the failed POST was not
/// sent. Preserve arbitrarily long valid waits; overflow means an indefinite
/// hold instead of shortening a provider-requested delay.
pub(super) fn not_before(headers: &HeaderMap, now: SystemTime) -> i64 {
    let mut values = headers.get_all(RETRY_AFTER).iter();
    let first = values.next();
    let parsed = if values.next().is_none() {
        first
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let value = value.trim();
                if value.is_empty() || value.len() > 128 {
                    return None;
                }
                if value.bytes().all(|byte| byte.is_ascii_digit()) {
                    let seconds = value.parse::<u64>().ok();
                    Some(seconds.and_then(|seconds| now.checked_add(Duration::from_secs(seconds))))
                } else {
                    httpdate::parse_http_date(value).ok().map(Some)
                }
            })
    } else {
        None
    };
    let target = match parsed {
        Some(Some(target)) => target,
        Some(None) => return i64::MAX,
        // Missing, ambiguous or invalid metadata imposes a conservative local
        // cooldown. The original send remains Unknown in every case.
        None => match now.checked_add(Duration::from_secs(60)) {
            Some(target) => target,
            None => return i64::MAX,
        },
    };
    let floor = now.checked_add(Duration::from_secs(1));
    target
        .max(floor.unwrap_or(target))
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_date_missing_and_oversized_delays_never_shorten_valid_waits() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut headers = HeaderMap::new();
        assert_eq!(not_before(&headers, now), 1_700_000_060_000);
        headers.insert(RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(not_before(&headers, now), 1_700_000_120_000);
        headers.insert(
            RETRY_AFTER,
            httpdate::fmt_http_date(now + Duration::from_secs(90))
                .parse()
                .unwrap(),
        );
        assert_eq!(not_before(&headers, now), 1_700_000_090_000);
        headers.insert(RETRY_AFTER, "864000".parse().unwrap());
        assert_eq!(not_before(&headers, now), 1_700_864_000_000);
        headers.insert(RETRY_AFTER, "18446744073709551616".parse().unwrap());
        assert_eq!(not_before(&headers, now), i64::MAX);
        headers.insert(RETRY_AFTER, "0".parse().unwrap());
        assert_eq!(not_before(&headers, now), 1_700_000_001_000);
        headers.append(RETRY_AFTER, "1000".parse().unwrap());
        assert_eq!(not_before(&headers, now), 1_700_000_060_000);
    }
}
