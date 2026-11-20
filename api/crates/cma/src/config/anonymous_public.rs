use std::num::NonZeroU32;

use super::positive_u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnonymousPublicConfig {
    pub(crate) enabled: bool,
    pub(crate) per_minute: NonZeroU32,
    pub(crate) concurrency: NonZeroU32,
}

pub(crate) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<AnonymousPublicConfig, String> {
    let enabled = match lookup("CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_ENABLED")?.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        _ => {
            return Err("CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_ENABLED must be true or false".into())
        }
    };
    Ok(AnonymousPublicConfig {
        enabled,
        per_minute: positive_u32(
            lookup,
            "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_PER_MINUTE",
            1_200,
            1,
        )?,
        concurrency: positive_u32(
            lookup,
            "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_CONCURRENCY",
            16,
            1,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_budget_is_disabled_by_default() {
        let config = from_lookup(&mut |_| Ok(None)).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.per_minute.get(), 1_200);
        assert_eq!(config.concurrency.get(), 16);
    }

    #[test]
    fn enabled_policy_is_strict_and_propagates_through_rate_configuration() {
        for value in [None, Some("false"), Some("true")] {
            let config = super::super::RateLimitConfig::from_lookup(&mut |key| {
                Ok(match key {
                    "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_ENABLED" => value.map(str::to_owned),
                    "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_PER_MINUTE" => Some("7".into()),
                    "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_CONCURRENCY" => Some("3".into()),
                    _ => None,
                })
            })
            .unwrap();
            assert_eq!(config.anonymous_public.enabled, value == Some("true"));
            assert_eq!(config.anonymous_public.per_minute.get(), 7);
            assert_eq!(config.anonymous_public.concurrency.get(), 3);
        }
        for invalid in ["", "1", "TRUE", "yes", " false "] {
            assert!(from_lookup(&mut |key| Ok((key
                == "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_ENABLED")
                .then(|| invalid.into())))
            .is_err());
        }
        assert_eq!(
            from_lookup(&mut |_| Err("lookup failed".into())).unwrap_err(),
            "lookup failed"
        );
    }

    #[test]
    fn both_limits_must_be_positive() {
        for key in [
            "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_PER_MINUTE",
            "CHRONO_SERVER_RATE_ANONYMOUS_PUBLIC_CONCURRENCY",
        ] {
            assert!(from_lookup(&mut |candidate| {
                Ok((candidate == key).then(|| "0".to_owned()))
            })
            .is_err());
        }
    }
}
