//! Speech-to-text spend settings: the metered seconds one call may cost, the
//! largest direct upload, the per-channel call rate, and the per-channel and
//! global hourly budgets of metered audio seconds.
//!
//! They are validated at startup even while speech-to-text is disabled, so
//! enabling it can never be the first moment a bad value is noticed.
use super::{
    media::{bounded, read_env},
    ConfigError,
};

pub const CMAEG_STT_MAX_SECONDS: &str = "CMAEG_STT_MAX_SECONDS";
pub const CMAEG_STT_MAX_BYTES: &str = "CMAEG_STT_MAX_BYTES";
pub const CMAEG_STT_RATE_PER_MINUTE: &str = "CMAEG_STT_RATE_PER_MINUTE";
pub const CMAEG_STT_CHANNEL_SECONDS_PER_HOUR: &str = "CMAEG_STT_CHANNEL_SECONDS_PER_HOUR";
pub const CMAEG_STT_SECONDS_PER_HOUR: &str = "CMAEG_STT_SECONDS_PER_HOUR";

const MAX_SECONDS_RANGE: (u64, u64) = (1, 1_500);
/// Below the speech-to-text API's 25 MB upload limit, leaving room for the
/// multipart framing around the file.
const MAX_BYTES_RANGE: (u64, u64) = (1_048_576, 24_000_000);
const RATE_PER_MINUTE_RANGE: (u64, u64) = (1, 600);
const CHANNEL_SECONDS_PER_HOUR_RANGE: (u64, u64) = (600, 86_400);
const SECONDS_PER_HOUR_RANGE: (u64, u64) = (600, 360_000);

/// Speech-to-text limits, loaded once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttConfig {
    /// Most metered seconds one transcription call may cost.
    pub max_seconds: u32,
    /// Most bytes one upload may carry.
    pub max_bytes: u64,
    /// Transcription calls per channel per minute.
    pub rate_per_minute: u32,
    /// Metered seconds per channel per hour.
    pub channel_seconds_per_hour: u32,
    /// Metered seconds per hour across every channel: the cost backstop.
    pub seconds_per_hour: u32,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            max_seconds: 600,
            max_bytes: 20_971_520,
            rate_per_minute: 20,
            channel_seconds_per_hour: 3_600,
            seconds_per_hour: 36_000,
        }
    }
}

impl SttConfig {
    /// Read every speech-to-text key through `lookup`, so tests need not
    /// mutate the process environment.
    ///
    /// One call must fit a channel's hour and a channel's hour must fit the
    /// global one, otherwise a single call or a single channel could never be
    /// admitted; startup refuses such a combination by naming all three keys.
    pub fn from_lookup(
        lookup: impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
    ) -> Result<Self, ConfigError> {
        let defaults = Self::default();
        let read = |key: &'static str, default: u64, (min, max): (u64, u64)| {
            bounded(key, lookup(key)?.as_deref(), default, min, max)
        };
        let config = Self {
            max_seconds: narrow(read(
                CMAEG_STT_MAX_SECONDS,
                u64::from(defaults.max_seconds),
                MAX_SECONDS_RANGE,
            )?),
            max_bytes: read(CMAEG_STT_MAX_BYTES, defaults.max_bytes, MAX_BYTES_RANGE)?,
            rate_per_minute: narrow(read(
                CMAEG_STT_RATE_PER_MINUTE,
                u64::from(defaults.rate_per_minute),
                RATE_PER_MINUTE_RANGE,
            )?),
            channel_seconds_per_hour: narrow(read(
                CMAEG_STT_CHANNEL_SECONDS_PER_HOUR,
                u64::from(defaults.channel_seconds_per_hour),
                CHANNEL_SECONDS_PER_HOUR_RANGE,
            )?),
            seconds_per_hour: narrow(read(
                CMAEG_STT_SECONDS_PER_HOUR,
                u64::from(defaults.seconds_per_hour),
                SECONDS_PER_HOUR_RANGE,
            )?),
        };
        if config.max_seconds > config.channel_seconds_per_hour
            || config.channel_seconds_per_hour > config.seconds_per_hour
        {
            return Err(ConfigError::InvalidMedia {
                detail: format!(
                    "{CMAEG_STT_MAX_SECONDS} ({}) <= {CMAEG_STT_CHANNEL_SECONDS_PER_HOUR} ({}) \
                     <= {CMAEG_STT_SECONDS_PER_HOUR} ({}) must hold",
                    config.max_seconds, config.channel_seconds_per_hour, config.seconds_per_hour
                ),
            });
        }
        tracing::info!(
            max_seconds = config.max_seconds,
            max_bytes = config.max_bytes,
            rate_per_minute = config.rate_per_minute,
            channel_seconds_per_hour = config.channel_seconds_per_hour,
            seconds_per_hour = config.seconds_per_hour,
            "speech-to-text spend limits"
        );
        Ok(config)
    }
}

/// Read the speech-to-text settings from the process environment.
pub(super) fn config_from_env() -> Result<SttConfig, ConfigError> {
    SttConfig::from_lookup(read_env)
}

/// Every key here is bounded far below `u32::MAX`, so narrowing cannot fail.
fn narrow(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
