use std::num::NonZeroU32;

use super::{
    anonymous_public::{self, AnonymousPublicConfig},
    positive_u32, rate_default_lease, MINIMUM_HEALTH_REQUESTS_PER_MINUTE,
};

/// Per-key request budgets enforced by this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub admission_per_minute: NonZeroU32,
    pub health_per_minute: NonZeroU32,
    pub default_per_minute: NonZeroU32,
    pub create_per_minute: NonZeroU32,
    pub input_per_minute: NonZeroU32,
    pub control_per_minute: NonZeroU32,
    pub control_tier_enabled: bool,
    pub response_input_tier_enabled: bool,
    pub anonymous_public: AnonymousPublicConfig,
    pub default_lease_enabled: bool,
}

impl RateLimitConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        Ok(Self {
            default_lease_enabled: rate_default_lease::enabled(lookup)?,
            control_tier_enabled: match lookup("CHRONO_SERVER_RATE_CONTROL_ENABLED")?.as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                _ => return Err("CHRONO_SERVER_RATE_CONTROL_ENABLED must be true or false".into()),
            },
            response_input_tier_enabled: match lookup("CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED")?
                .as_deref()
            {
                None | Some("false") => false,
                Some("true") => true,
                _ => {
                    return Err(
                        "CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED must be true or false".into(),
                    )
                }
            },
            admission_per_minute: positive_u32(
                lookup,
                "CHRONO_SERVER_RATE_ADMISSION_PER_MINUTE",
                6_000,
                1,
            )?,
            health_per_minute: positive_u32(
                lookup,
                "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
                120,
                MINIMUM_HEALTH_REQUESTS_PER_MINUTE,
            )?,
            default_per_minute: positive_u32(
                lookup,
                "CHRONO_SERVER_RATE_DEFAULT_PER_MINUTE",
                600,
                1,
            )?,
            create_per_minute: positive_u32(lookup, "CHRONO_SERVER_RATE_CREATE_PER_MINUTE", 10, 1)?,
            input_per_minute: positive_u32(lookup, "CHRONO_SERVER_RATE_INPUT_PER_MINUTE", 120, 1)?,
            control_per_minute: positive_u32(
                lookup,
                "CHRONO_SERVER_RATE_CONTROL_PER_MINUTE",
                120,
                1,
            )?,
            anonymous_public: anonymous_public::from_lookup(lookup)?,
        })
    }
}
