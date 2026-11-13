use std::num::NonZeroU32;

use super::positive_u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnonymousPublicConfig {
    pub(crate) per_minute: NonZeroU32,
    pub(crate) concurrency: NonZeroU32,
}

pub(crate) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<AnonymousPublicConfig, String> {
    Ok(AnonymousPublicConfig {
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
    fn defaults_match_the_temporary_fleet_budget() {
        let config = from_lookup(&mut |_| Ok(None)).unwrap();
        assert_eq!(config.per_minute.get(), 1_200);
        assert_eq!(config.concurrency.get(), 16);
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
