//! Speech-to-text settings: the endpoint, model, formats and timeout of the
//! transcriber, and the spend limits every call runs under (the metered
//! seconds one call may cost, the largest direct upload, the per-channel call
//! rate, and the per-channel and global hourly budgets of metered seconds).
//!
//! They are validated at startup even while speech-to-text is disabled, so
//! enabling it can never be the first moment a bad value is noticed. An empty
//! `CMAEG_STT_URL` disables it, and the platform key is then ignored.
use super::{
    media::{bounded, read_env},
    ConfigError,
};
use std::{collections::BTreeSet, fmt, time::Duration};

mod endpoint;
pub use endpoint::SttEndpoint;

pub const CMAEG_STT_URL: &str = "CMAEG_STT_URL";
pub const CMAEG_STT_MODEL: &str = "CMAEG_STT_MODEL";
pub const CMAEG_STT_FORMATS: &str = "CMAEG_STT_FORMATS";
pub const CMAEG_STT_TIMEOUT_MS: &str = "CMAEG_STT_TIMEOUT_MS";
pub const CMAEG_STT_API_KEY: &str = "CMAEG_STT_API_KEY";
pub const CMAEG_STT_MAX_SECONDS: &str = "CMAEG_STT_MAX_SECONDS";
pub const CMAEG_STT_MAX_BYTES: &str = "CMAEG_STT_MAX_BYTES";
pub const CMAEG_STT_RATE_PER_MINUTE: &str = "CMAEG_STT_RATE_PER_MINUTE";
pub const CMAEG_STT_CHANNEL_SECONDS_PER_HOUR: &str = "CMAEG_STT_CHANNEL_SECONDS_PER_HOUR";
pub const CMAEG_STT_SECONDS_PER_HOUR: &str = "CMAEG_STT_SECONDS_PER_HOUR";

/// Every format a speech-to-text endpoint may be configured to accept.
/// `mpeg` and `mpga` are the API's aliases for MP3; `ogg` and `flac` are
/// opt-in until a probe shows the endpoint takes them.
pub const KNOWN_FORMATS: [&str; 9] = [
    "mp3", "mp4", "mpeg", "mpga", "m4a", "wav", "webm", "ogg", "flac",
];
const MAX_SECONDS_RANGE: (u64, u64) = (1, 1_500);
/// Below the speech-to-text API's 25 MB upload limit, leaving room for the
/// multipart framing around the file.
const MAX_BYTES_RANGE: (u64, u64) = (1_048_576, 24_000_000);
const RATE_PER_MINUTE_RANGE: (u64, u64) = (1, 600);
const CHANNEL_SECONDS_PER_HOUR_RANGE: (u64, u64) = (600, 86_400);
const SECONDS_PER_HOUR_RANGE: (u64, u64) = (600, 360_000);

/// Speech-to-text settings, loaded once at startup.
#[derive(Clone, PartialEq, Eq)]
pub struct SttConfig {
    /// Where transcriptions are sent; `None` while speech-to-text is disabled.
    pub endpoint: Option<SttEndpoint>,
    pub model: String,
    /// The format names the endpoint accepts, from [`KNOWN_FORMATS`].
    pub formats: BTreeSet<&'static str>,
    /// The deadline of one request attempt.
    pub timeout: Duration,
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
            endpoint: None,
            model: endpoint::DEFAULT_MODEL.to_owned(),
            formats: endpoint::DEFAULT_FORMATS.into_iter().collect(),
            timeout: Duration::from_millis(endpoint::DEFAULT_TIMEOUT_MS),
            max_seconds: 600,
            max_bytes: 20_971_520,
            rate_per_minute: 20,
            channel_seconds_per_hour: 3_600,
            seconds_per_hour: 36_000,
        }
    }
}

/// Only whether an endpoint is configured: its URL and key never reach a log.
impl fmt::Debug for SttConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SttConfig")
            .field("url_configured", &self.endpoint.is_some())
            .field("model", &self.model)
            .field("formats", &self.formats)
            .field("timeout", &self.timeout)
            .field("max_seconds", &self.max_seconds)
            .field("max_bytes", &self.max_bytes)
            .field("rate_per_minute", &self.rate_per_minute)
            .field("channel_seconds_per_hour", &self.channel_seconds_per_hour)
            .field("seconds_per_hour", &self.seconds_per_hour)
            .finish()
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
        let mut config = Self {
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
            ..defaults
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
        let settings = endpoint::read(&lookup)?;
        config.endpoint = settings.endpoint;
        config.model = settings.model;
        config.formats = settings.formats;
        config.timeout = settings.timeout;
        tracing::info!(
            enabled = config.endpoint.is_some(),
            model = %config.model,
            formats = ?config.formats,
            timeout_ms = config.timeout.as_millis() as u64,
            max_seconds = config.max_seconds,
            max_bytes = config.max_bytes,
            rate_per_minute = config.rate_per_minute,
            channel_seconds_per_hour = config.channel_seconds_per_hour,
            seconds_per_hour = config.seconds_per_hour,
            "speech-to-text settings"
        );
        Ok(config)
    }
}

/// Read the speech-to-text settings from the process environment. The
/// platform key goes from the OS string straight into the value that becomes
/// its [`Secret`](cmaeg_core::channel::Secret), and no error ever carries it.
pub(super) fn config_from_env() -> Result<SttConfig, ConfigError> {
    SttConfig::from_lookup(|key| {
        if key != CMAEG_STT_API_KEY {
            return read_env(key);
        }
        std::env::var_os(key)
            .map(|value| {
                value.into_string().map_err(|_| ConfigError::InvalidMedia {
                    detail: format!("{CMAEG_STT_API_KEY} must be printable ASCII"),
                })
            })
            .transpose()
    })
}

/// Every key here is bounded far below `u32::MAX`, so narrowing cannot fail.
fn narrow(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
