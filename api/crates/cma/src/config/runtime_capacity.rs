//! Behavior-neutral defaults; activation values belong in the reviewed scale profile.
use layer3_adapter::agent_sandbox::RuntimeCapacityConfig;
use std::time::Duration;

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<RuntimeCapacityConfig, String> {
    let config = RuntimeCapacityConfig {
        stream_limit: number(lookup, "CHRONO_RUNTIME_STREAM_LIMIT", 64, 1, 4096)?,
        request_limit: number(lookup, "CHRONO_RUNTIME_REQUEST_LIMIT", 32, 1, 4096)?,
        stream_reserve: number(lookup, "CHRONO_RUNTIME_STREAM_RESERVE", 0, 0, 4095)?,
        request_reserve: number(lookup, "CHRONO_RUNTIME_REQUEST_RESERVE", 0, 0, 4095)?,
        request_queue_limit: number(lookup, "CHRONO_RUNTIME_REQUEST_QUEUE_LIMIT", 0, 0, 1024)?,
        request_queue_max_wait: Duration::from_millis(number(
            lookup,
            "CHRONO_RUNTIME_REQUEST_QUEUE_MAX_WAIT_MS",
            1000,
            1,
            60000,
        )? as u64),
    };
    config
        .validate()
        .map_err(|error| format!("CHRONO_RUNTIME capacity configuration: {error}"))
}

pub(super) fn number(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let Some(value) = lookup(key)? else {
        return Ok(default);
    };
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<usize>().ok())
        .flatten()
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{key} must be an integer from {min} through {max}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_activation_are_explicit() {
        assert_eq!(
            from_lookup(&mut |_| Ok(None)).unwrap(),
            RuntimeCapacityConfig::default()
        );
        let config = from_lookup(&mut |key| {
            Ok(match key {
                "CHRONO_RUNTIME_STREAM_RESERVE" => Some("16".into()),
                "CHRONO_RUNTIME_REQUEST_RESERVE" => Some("8".into()),
                "CHRONO_RUNTIME_REQUEST_QUEUE_LIMIT" => Some("64".into()),
                _ => None,
            })
        })
        .unwrap();
        assert_eq!(
            (config.usable_streams(), config.usable_requests()),
            (48, 24)
        );
        assert_eq!(config.request_queue_max_wait, Duration::from_secs(1));
    }
    #[test]
    fn invalid_config_fails_before_startup() {
        for key in [
            "CHRONO_RUNTIME_STREAM_LIMIT",
            "CHRONO_RUNTIME_REQUEST_LIMIT",
            "CHRONO_RUNTIME_STREAM_RESERVE",
            "CHRONO_RUNTIME_REQUEST_RESERVE",
            "CHRONO_RUNTIME_REQUEST_QUEUE_LIMIT",
            "CHRONO_RUNTIME_REQUEST_QUEUE_MAX_WAIT_MS",
        ] {
            for value in [
                "",
                "-1",
                "+1",
                " 1",
                "1 ",
                "1.5",
                "true",
                "9999999999999999999999999",
            ] {
                assert!(
                    from_lookup(&mut |name| Ok((name == key).then(|| value.into()))).is_err(),
                    "{key}={value}"
                );
            }
        }
        for (key, value) in [
            ("CHRONO_RUNTIME_STREAM_LIMIT", "0"),
            ("CHRONO_RUNTIME_REQUEST_LIMIT", "4097"),
            ("CHRONO_RUNTIME_STREAM_RESERVE", "64"),
            ("CHRONO_RUNTIME_REQUEST_RESERVE", "32"),
            ("CHRONO_RUNTIME_REQUEST_QUEUE_LIMIT", "1025"),
            ("CHRONO_RUNTIME_REQUEST_QUEUE_MAX_WAIT_MS", "60001"),
            ("CHRONO_RUNTIME_REQUEST_QUEUE_MAX_WAIT_MS", "0"),
        ] {
            assert!(from_lookup(&mut |name| Ok((name == key).then(|| value.into()))).is_err());
        }
        assert!(from_lookup(&mut |_| Err("lookup failed".into())).is_err());
    }
}
