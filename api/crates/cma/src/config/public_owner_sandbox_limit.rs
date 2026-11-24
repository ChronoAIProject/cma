use std::num::NonZeroU16;

const ENABLED: &str = "CHRONO_PUBLIC_OWNER_SANDBOX_LIMIT_ENABLED";
const LIMIT: &str = "CHRONO_PUBLIC_OWNER_SANDBOX_LIMIT";
const DEFAULT_LIMIT: NonZeroU16 = NonZeroU16::new(2).unwrap();

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<NonZeroU16>, String> {
    let enabled = super::features::flag(lookup, ENABLED)?;
    let limit = match lookup(LIMIT)? {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .parse::<NonZeroU16>()
            .ok()
            .filter(|_| value.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| format!("{LIMIT} must be an integer from 1 to 65535"))?,
    };
    Ok(enabled.then_some(limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(enabled: Option<&str>, limit: Option<&str>) -> Result<Option<u16>, String> {
        from_lookup(&mut |key| {
            Ok(match key {
                ENABLED => enabled,
                LIMIT => limit,
                _ => panic!("unexpected configuration key {key}"),
            }
            .map(str::to_owned))
        })
        .map(|limit| limit.map(NonZeroU16::get))
    }

    #[test]
    fn defaults_to_two_only_after_explicit_activation() {
        assert_eq!(parse(None, None).unwrap(), None);
        assert_eq!(parse(None, Some("3")).unwrap(), None);
        assert_eq!(parse(Some("false"), Some("3")).unwrap(), None);
        assert_eq!(parse(Some("true"), None).unwrap(), Some(2));
        assert_eq!(parse(Some("true"), Some("1")).unwrap(), Some(1));
        assert_eq!(parse(Some("true"), Some("65535")).unwrap(), Some(65535));
    }

    #[test]
    fn rejects_invalid_flags_and_limits() {
        assert!(parse(Some("TRUE"), None).unwrap_err().contains(ENABLED));
        for invalid in ["", "0", "-1", "+1", " 2", "2.0", "65536"] {
            assert!(parse(Some("true"), Some(invalid))
                .unwrap_err()
                .contains(LIMIT));
        }
    }
}
