//! The media byte, job and rate budgets: bounds, defaults and parsing.
use super::{
    ConfigError, MediaConfig, CMAEG_MEDIA_CACHE_BYTES, CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
    CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_LINK_TTL_SECONDS, CMAEG_MEDIA_MAX_BYTES,
    CMAEG_MEDIA_MAX_JOBS, CMAEG_MEDIA_RATE_PER_MINUTE, CMAEG_MEDIA_RESPONSE_BYTES, MIB,
};

/// Above Telegram's and NyxID's 20 MiB source limits, with headroom for a
/// custom origin; one original is held whole while it is processed.
const MAX_BYTES_CEILING: u64 = 25 * MIB;
const INFLIGHT_BYTES_CEILING: u64 = 1024 * MIB;
const RESPONSE_BYTES_RANGE: (u64, u64) = (4 * MIB, 128 * MIB);
const MAX_JOBS_RANGE: (u64, u64) = (1, 64);
const CACHE_BYTES_CEILING: u64 = 256 * MIB;
const RATE_PER_MINUTE_RANGE: (u64, u64) = (1, 6_000);
/// At least one maximum-size original per hour, at most 4 GiB.
const DOWNLOAD_BYTES_PER_HOUR_RANGE: (u64, u64) = (20 * MIB, 4_096 * MIB);
/// Long enough to run the command, never past the sealed maximum.
const LINK_TTL_SECONDS_RANGE: (u64, u64) = (30, 600);

/// The budget keys, read through `lookup` so tests need not mutate the
/// process environment. The in-flight floor depends on the per-attachment
/// maximum: one original must always fit.
pub(super) fn budgets(
    lookup: impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
) -> Result<MediaConfig, ConfigError> {
    let defaults = MediaConfig::default();
    let read = |key: &'static str, default: u64, (min, max): (u64, u64)| {
        bounded(key, lookup(key)?.as_deref(), default, min, max)
    };
    let max_bytes = read(
        CMAEG_MEDIA_MAX_BYTES,
        defaults.max_bytes,
        (1, MAX_BYTES_CEILING),
    )?;
    let config = MediaConfig {
        max_bytes,
        inflight_bytes: read(
            CMAEG_MEDIA_INFLIGHT_BYTES,
            defaults.inflight_bytes,
            (max_bytes, INFLIGHT_BYTES_CEILING),
        )?,
        response_bytes: read(
            CMAEG_MEDIA_RESPONSE_BYTES,
            defaults.response_bytes,
            RESPONSE_BYTES_RANGE,
        )?,
        max_jobs: narrow(read(
            CMAEG_MEDIA_MAX_JOBS,
            u64::from(defaults.max_jobs),
            MAX_JOBS_RANGE,
        )?),
        cache_bytes: read(
            CMAEG_MEDIA_CACHE_BYTES,
            defaults.cache_bytes,
            (0, CACHE_BYTES_CEILING),
        )?,
        rate_per_minute: narrow(read(
            CMAEG_MEDIA_RATE_PER_MINUTE,
            u64::from(defaults.rate_per_minute),
            RATE_PER_MINUTE_RANGE,
        )?),
        download_bytes_per_hour: read(
            CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
            defaults.download_bytes_per_hour,
            DOWNLOAD_BYTES_PER_HOUR_RANGE,
        )?,
        link_ttl_seconds: read(
            CMAEG_MEDIA_LINK_TTL_SECONDS,
            defaults.link_ttl_seconds,
            LINK_TTL_SECONDS_RANGE,
        )?,
        ..defaults
    };
    tracing::info!(
        max_bytes = config.max_bytes,
        inflight_bytes = config.inflight_bytes,
        response_bytes = config.response_bytes,
        max_jobs = config.max_jobs,
        cache_bytes = config.cache_bytes,
        rate_per_minute = config.rate_per_minute,
        download_bytes_per_hour = config.download_bytes_per_hour,
        link_ttl_seconds = config.link_ttl_seconds,
        "inbound media budgets"
    );
    Ok(config)
}

/// An unset key takes its default; a set one must be plain ASCII digits (no
/// sign, space or separator) within `min..=max`.
pub(in crate::config) fn bounded(
    key: &'static str,
    value: Option<&str>,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse::<u64>().ok())
        .flatten()
        .filter(|parsed| (min..=max).contains(parsed))
        .ok_or_else(|| ConfigError::InvalidMedia {
            detail: format!("{key} must be an unsigned decimal from {min} to {max}"),
        })
}

/// The u32 keys have bounds far below `u32::MAX`, so narrowing cannot fail.
fn narrow(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(error: ConfigError) -> String {
        match error {
            ConfigError::InvalidMedia { detail } => detail,
            other => panic!("expected an invalid media configuration, got {other}"),
        }
    }

    fn budgets_with(pairs: &[(&'static str, &str)]) -> Result<MediaConfig, ConfigError> {
        let pairs: Vec<(&'static str, String)> = pairs
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned()))
            .collect();
        budgets(|key| {
            Ok(pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone()))
        })
    }

    #[test]
    fn unset_budgets_take_their_documented_defaults() {
        let config = budgets_with(&[]).unwrap();
        assert_eq!(config.max_bytes, 20_971_520);
        assert_eq!(config.inflight_bytes, 67_108_864);
        assert_eq!(config.response_bytes, 25_165_824);
        assert_eq!(config.max_jobs, 6);
        assert_eq!(config.cache_bytes, 33_554_432);
        assert_eq!(config.rate_per_minute, 60);
        assert_eq!(config.download_bytes_per_hour, 268_435_456);
        assert_eq!(config.link_ttl_seconds, 120);
        assert!(!config.enabled);
    }

    #[test]
    fn budgets_accept_their_bounds_inclusive() {
        let config = budgets_with(&[
            (CMAEG_MEDIA_MAX_BYTES, "26214400"),
            (CMAEG_MEDIA_INFLIGHT_BYTES, "26214400"),
            (CMAEG_MEDIA_RESPONSE_BYTES, "4194304"),
            (CMAEG_MEDIA_MAX_JOBS, "64"),
            (CMAEG_MEDIA_CACHE_BYTES, "0"),
            (CMAEG_MEDIA_RATE_PER_MINUTE, "6000"),
            (CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR, "4294967296"),
            (CMAEG_MEDIA_LINK_TTL_SECONDS, "600"),
        ])
        .unwrap();
        assert_eq!(config.max_bytes, 26_214_400);
        assert_eq!(config.inflight_bytes, 26_214_400);
        assert_eq!(config.response_bytes, 4_194_304);
        assert_eq!(config.max_jobs, 64);
        assert_eq!(config.cache_bytes, 0);
        assert_eq!(config.rate_per_minute, 6_000);
        assert_eq!(config.download_bytes_per_hour, 4_294_967_296);
        assert_eq!(config.link_ttl_seconds, 600);
        let config = budgets_with(&[
            (CMAEG_MEDIA_MAX_BYTES, "1"),
            (CMAEG_MEDIA_INFLIGHT_BYTES, "1073741824"),
            (CMAEG_MEDIA_RESPONSE_BYTES, "134217728"),
            (CMAEG_MEDIA_MAX_JOBS, "1"),
            (CMAEG_MEDIA_CACHE_BYTES, "268435456"),
            (CMAEG_MEDIA_RATE_PER_MINUTE, "1"),
            (CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR, "20971520"),
            (CMAEG_MEDIA_LINK_TTL_SECONDS, "30"),
        ])
        .unwrap();
        assert_eq!(config.max_bytes, 1);
        assert_eq!(config.link_ttl_seconds, 30);
        assert_eq!(config.inflight_bytes, 1_073_741_824);
        assert_eq!(config.cache_bytes, 268_435_456);
    }

    #[test]
    fn out_of_bounds_or_malformed_budgets_name_their_key() {
        for (key, value, range) in [
            (CMAEG_MEDIA_MAX_BYTES, "0", "from 1 to 26214400"),
            (CMAEG_MEDIA_MAX_BYTES, "26214401", "from 1 to 26214400"),
            (
                CMAEG_MEDIA_RESPONSE_BYTES,
                "4194303",
                "from 4194304 to 134217728",
            ),
            (
                CMAEG_MEDIA_RESPONSE_BYTES,
                "134217729",
                "from 4194304 to 134217728",
            ),
            (CMAEG_MEDIA_MAX_JOBS, "0", "from 1 to 64"),
            (CMAEG_MEDIA_MAX_JOBS, "65", "from 1 to 64"),
            (CMAEG_MEDIA_CACHE_BYTES, "268435457", "from 0 to 268435456"),
            (CMAEG_MEDIA_RATE_PER_MINUTE, "0", "from 1 to 6000"),
            (CMAEG_MEDIA_RATE_PER_MINUTE, "6001", "from 1 to 6000"),
            (
                CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
                "20971519",
                "from 20971520 to 4294967296",
            ),
            (
                CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
                "4294967297",
                "from 20971520 to 4294967296",
            ),
            (CMAEG_MEDIA_LINK_TTL_SECONDS, "29", "from 30 to 600"),
            (CMAEG_MEDIA_LINK_TTL_SECONDS, "601", "from 30 to 600"),
        ] {
            assert_eq!(
                detail(budgets_with(&[(key, value)]).unwrap_err()),
                format!("{key} must be an unsigned decimal {range}"),
                "{key}={value}"
            );
        }
        for value in [
            "",
            " 60",
            "60 ",
            "+60",
            "-1",
            "6e1",
            "0x3c",
            "1_000",
            "99999999999999999999999",
        ] {
            let error = budgets_with(&[(CMAEG_MEDIA_RATE_PER_MINUTE, value)]).unwrap_err();
            assert_eq!(
                detail(error),
                "CMAEG_MEDIA_RATE_PER_MINUTE must be an unsigned decimal from 1 to 6000",
                "{value:?}"
            );
        }
    }

    #[test]
    fn the_inflight_floor_follows_the_per_attachment_maximum() {
        assert_eq!(
            detail(
                budgets_with(&[
                    (CMAEG_MEDIA_MAX_BYTES, "1048576"),
                    (CMAEG_MEDIA_INFLIGHT_BYTES, "1048575"),
                ])
                .unwrap_err()
            ),
            "CMAEG_MEDIA_INFLIGHT_BYTES must be an unsigned decimal from 1048576 to 1073741824"
        );
        // The default in-flight budget sits below a raised maximum.
        assert!(budgets_with(&[(CMAEG_MEDIA_MAX_BYTES, "26214400")]).is_ok());
        assert_eq!(
            detail(
                budgets_with(&[
                    (CMAEG_MEDIA_MAX_BYTES, "26214400"),
                    (CMAEG_MEDIA_INFLIGHT_BYTES, "20971520"),
                ])
                .unwrap_err()
            ),
            "CMAEG_MEDIA_INFLIGHT_BYTES must be an unsigned decimal from 26214400 to 1073741824"
        );
    }

    #[test]
    fn a_lookup_failure_is_reported_unchanged() {
        let error = budgets(|key| {
            Err(ConfigError::InvalidMedia {
                detail: format!("{key} must be valid UTF-8"),
            })
        })
        .unwrap_err();
        assert_eq!(detail(error), "CMAEG_MEDIA_MAX_BYTES must be valid UTF-8");
    }
}
