//! Default-tier prepayment is an independent, explicit opt-in.

const KEY: &str = "CHRONO_SERVER_RATE_DEFAULT_LEASE_ENABLED";

pub(super) fn enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup(KEY)?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{KEY} must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_switch_defaults_off_and_accepts_only_exact_booleans() {
        for (raw, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let config = super::super::RateLimitConfig::from_lookup(&mut |key| {
                Ok((key == KEY).then_some(raw).flatten().map(str::to_owned))
            })
            .unwrap();
            assert_eq!(config.default_lease_enabled, expected);
            assert_eq!(config.default_per_minute.get(), 600);
            assert!(!config.control_tier_enabled);
        }
        for value in ["", "TRUE", "1", " true ", "yes"] {
            assert!(enabled(&mut |_| Ok(Some(value.into())))
                .unwrap_err()
                .contains(KEY));
        }
        assert_eq!(
            enabled(&mut |_| Err("lookup failed".into())),
            Err("lookup failed".into())
        );
    }
}
