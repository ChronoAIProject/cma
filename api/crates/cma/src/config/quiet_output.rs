//! Strict rollout settings for quiet receipt checkpoint coalescing.
use layer2_core::sandbox::AgentOutputCoalescePolicy;
use std::time::Duration;

const ENABLED: &str = "CHRONO_AGENT_OUTPUT_QUIET_COALESCE_ENABLED";
const MILLIS: &str = "CHRONO_AGENT_OUTPUT_QUIET_COALESCE_MAX_MS";
const FRAMES: &str = "CHRONO_AGENT_OUTPUT_QUIET_COALESCE_MAX_FRAMES";

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<AgentOutputCoalescePolicy>, String> {
    let enabled = match lookup(ENABLED)?.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        _ => return Err(format!("{ENABLED} must be true or false")),
    };
    let millis = bound(lookup, MILLIS, 250, 1000)?;
    let frames = bound(lookup, FRAMES, 64, 128)?;
    let policy = AgentOutputCoalescePolicy::new(Duration::from_millis(millis), frames as usize)
        .map_err(|error| error.to_string())?;
    Ok(enabled.then_some(policy))
}

fn bound(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: u64,
    maximum: u64,
) -> Result<u64, String> {
    let Some(value) = lookup(key)? else {
        return Ok(default);
    };
    let invalid = || format!("{key} must be an integer in 1..={maximum}");
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|value| (1..=maximum).contains(value))
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_off_and_strict_enabled_bounds() {
        assert_eq!(from_lookup(&mut |_| Ok(None)).unwrap(), None);
        assert_eq!(
            from_lookup(&mut |key| Ok((key == ENABLED).then(|| "true".into()))).unwrap(),
            Some(AgentOutputCoalescePolicy::new(Duration::from_millis(250), 64).unwrap())
        );
        for (key, value) in [
            (MILLIS, "1000"),
            (FRAMES, "128"),
            (MILLIS, "1"),
            (FRAMES, "1"),
        ] {
            assert!(from_lookup(&mut |name| Ok(if name == key {
                Some(value.into())
            } else {
                None
            }))
            .is_ok());
        }
    }

    #[test]
    fn invalid_bounds_fail_even_when_disabled_and_lookup_errors_propagate() {
        for (key, value) in [
            (ENABLED, "TRUE"),
            (ENABLED, "1"),
            (MILLIS, "1001"),
            (FRAMES, "129"),
            (MILLIS, "0"),
            (FRAMES, "0"),
            (FRAMES, "-1"),
            (MILLIS, " 250"),
            (MILLIS, "250ms"),
            (FRAMES, "+64"),
            (FRAMES, ""),
            (MILLIS, "18446744073709551616"),
        ] {
            let error =
                from_lookup(&mut |name| Ok((name == key).then(|| value.into()))).unwrap_err();
            assert!(error.contains(key), "{key}: {error}");
        }
        assert_eq!(
            from_lookup(&mut |_| Err("lookup failed".into())).unwrap_err(),
            "lookup failed"
        );
    }
}
