//! Independent, default-off public observation admission.

pub(super) fn event_fallback(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<std::time::Duration, String> {
    const KEY: &str = "CHRONO_AGENT_EVENT_STREAM_FALLBACK_POLL_MS";
    let Some(value) = lookup(KEY)? else {
        return Ok(std::time::Duration::from_secs(1));
    };
    let millis = (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<u32>().ok())
        .flatten()
        .filter(|value| *value >= 100)
        .ok_or_else(|| format!("{KEY} must be an integer from 100 through {}", u32::MAX))?;
    Ok(std::time::Duration::from_millis(u64::from(millis)))
}

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<usize, String> {
    const KEY: &str = "CHRONO_PUBLIC_STREAM_LIMIT";
    let Some(value) = lookup(KEY)? else {
        return Ok(0);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{KEY} must be an integer from 0 through 4096"));
    }
    value
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit <= 4096)
        .ok_or_else(|| format!("{KEY} must be an integer from 0 through 4096"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wake_defaults_and_fallback_are_strict() {
        use super::super::features::FeatureFlags;
        let flags = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
        assert!(!flags.agent_event_stream_wake);
        assert_eq!(flags.agent_event_stream_fallback.as_millis(), 1000);
        for value in ["100", "1000", "4294967295"] {
            assert_eq!(
                event_fallback(&mut |_| Ok(Some(value.into())))
                    .unwrap()
                    .as_millis(),
                value.parse::<u128>().unwrap()
            );
        }
        for value in [
            "",
            "99",
            "-100",
            "+100",
            " 100",
            "100 ",
            "100.0",
            "4294967296",
        ] {
            assert!(
                event_fallback(&mut |_| Ok(Some(value.into()))).is_err(),
                "{value}"
            );
        }
        assert!(event_fallback(&mut |_| Err("lookup failed".into())).is_err());
        for (value, expected) in [
            ("true", Some(true)),
            ("false", Some(false)),
            ("TRUE", None),
            ("1", None),
        ] {
            let result = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_AGENT_EVENT_STREAM_WAKE_ENABLED").then(|| value.into()))
            });
            assert_eq!(
                result.ok().map(|flags| flags.agent_event_stream_wake),
                expected
            );
        }
    }

    #[test]
    fn public_stream_admission_is_off_by_default_and_strictly_bounded() {
        assert_eq!(from_lookup(&mut |_| Ok(None)).unwrap(), 0);
        for value in ["0", "1", "64", "4096"] {
            assert_eq!(
                from_lookup(&mut |_| Ok(Some(value.into()))).unwrap(),
                value.parse::<usize>().unwrap()
            );
        }
        for value in [
            "",
            "-1",
            "+1",
            " 1",
            "1 ",
            "true",
            "1.0",
            "4097",
            "9999999999999999999999",
        ] {
            assert!(from_lookup(&mut |_| Ok(Some(value.into()))).is_err());
        }
        assert!(from_lookup(&mut |_| Err("lookup failed".into())).is_err());
    }
}
