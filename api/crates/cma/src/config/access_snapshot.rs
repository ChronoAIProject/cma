//! Independent authorization reader rollout; hierarchy controls stay separate.

const KEY: &str = "CHRONO_ACCESS_SNAPSHOT_SESSION_ENABLED";

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
    use crate::config::FeatureFlags;

    #[test]
    fn access_snapshot_is_independent_and_defaults_off() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then_some(value).flatten().map(str::to_owned))
            })
            .unwrap();
            assert_eq!(flags.access_snapshot_session, expected);
            assert!(!flags.sandbox_snapshot_reads);
            assert!(!flags.sandbox_batched_reads);
        }
    }

    #[test]
    fn access_snapshot_rejects_invalid_values_and_lookup_failures() {
        for value in ["", "1", "0", "TRUE", "False", " true "] {
            assert_eq!(
                enabled(&mut |_| Ok(Some(value.into()))),
                Err(format!("{KEY} must be true or false"))
            );
        }
        assert_eq!(
            enabled(&mut |_| Err("lookup failed".into())),
            Err("lookup failed".into())
        );
    }
}
