//! Environment configuration for the CMA Event Gateway.

use std::{env, net::SocketAddr, sync::Arc};

use crate::providers::ProviderRegistry;
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use reqwest::Url;

mod cache;
mod delivery;
mod ingress;
mod media;
mod secrets;
mod service_account;
mod stt;
pub mod transformer;
mod turns;
pub use cache::*;
pub use delivery::*;
pub use ingress::*;
pub use media::{
    MediaConfig, MediaToolProviders, CMAEG_MEDIA_CACHE_BYTES, CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
    CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_MAX_BYTES, CMAEG_MEDIA_MAX_JOBS,
    CMAEG_MEDIA_PREPARE_BUDGET_MS, CMAEG_MEDIA_RATE_PER_MINUTE, CMAEG_MEDIA_RESPONSE_BYTES,
    CMAEG_MEDIA_TOOL_PROVIDERS,
};
pub use service_account::*;
pub use stt::*;
pub use transformer::{TransformerConfig, CMAEG_MEDIA_FFMPEG_DIR, CMAEG_MEDIA_VIDEO_FRAMES};
pub use turns::*;

/// The listener address used by CMAEG.
pub const CMAEG_LISTEN: &str = "CMAEG_LISTEN";
pub const CMAEG_METRICS_LISTEN: &str = "CMAEG_METRICS_LISTEN";
pub const CMAEG_PROVIDERS: &str = "CMAEG_PROVIDERS";
pub const CMAEG_PROVIDER_CONNECT_TIMEOUT_MS: &str = "CMAEG_PROVIDER_CONNECT_TIMEOUT_MS";
pub const CMAEG_PROVIDER_REQUEST_TIMEOUT_MS: &str = "CMAEG_PROVIDER_REQUEST_TIMEOUT_MS";
pub const CMAEG_PROVIDER_MAX_INFLIGHT: &str = "CMAEG_PROVIDER_MAX_INFLIGHT";
pub const CMAEG_PUBLIC_URL: &str = "CMAEG_PUBLIC_URL";
pub const CMAEG_MASTER_KEYS: &str = "CMAEG_MASTER_KEYS";
pub const CMAEG_MASTER_KEYS_FILE: &str = "CMAEG_MASTER_KEYS_FILE";
pub const CMAEG_CMA_BASE_URL: &str = "CMAEG_CMA_BASE_URL";
pub const CMAEG_CMA_SERVICE_TOKEN: &str = "CMAEG_CMA_SERVICE_TOKEN";
pub const CMAEG_CMA_SERVICE_TOKEN_FILE: &str = "CMAEG_CMA_SERVICE_TOKEN_FILE";
pub const CMAEG_NYXID_BASE_URL: &str = "CMAEG_NYXID_BASE_URL";
pub const CMAEG_TOOLS_NYXID_SLUG: &str = "CMAEG_TOOLS_NYXID_SLUG";

/// Configuration errors that prevent the service from binding safely.
#[derive(Debug)]
pub enum ConfigError {
    /// The configured listener was not a valid socket address.
    InvalidListen {
        name: &'static str,
        value: String,
        source: std::net::AddrParseError,
    },
    /// The provider registry JSON did not satisfy the outbound-call policy.
    InvalidProviders { detail: String },
    /// A duration setting was not a positive bounded integer.
    InvalidDuration { name: &'static str, value: String },
    /// The global provider concurrency cap was invalid.
    InvalidMaxInflight { value: String },
    /// A management-plane origin, secret, or slug was invalid.
    InvalidManagement { detail: String },
    /// An inbound media setting was invalid.
    InvalidMedia { detail: String },
    /// The configured media transformer failed its startup checks.
    MediaTransformer { detail: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidListen {
                name,
                value,
                source,
            } => {
                write!(formatter, "{name}={value:?} is invalid: {source}")
            }
            Self::InvalidProviders { detail } => {
                write!(formatter, "{CMAEG_PROVIDERS} is invalid: {detail}")
            }
            Self::InvalidDuration { name, value } => {
                write!(
                    formatter,
                    "{name}={value:?} is invalid: expected a positive millisecond value"
                )
            }
            Self::InvalidMaxInflight { value } => {
                write!(formatter, "{CMAEG_PROVIDER_MAX_INFLIGHT}={value:?} is invalid: expected a positive integer")
            }
            Self::InvalidManagement { detail } => {
                write!(formatter, "management configuration is invalid: {detail}")
            }
            Self::InvalidMedia { detail } => {
                write!(formatter, "media configuration is invalid: {detail}")
            }
            Self::MediaTransformer { detail } => {
                write!(formatter, "media transformer unavailable: {detail}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Process configuration loaded once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address on which the HTTP service accepts requests.
    pub listen: SocketAddr,
    pub metrics_listen: SocketAddr,
    /// Providers explicitly allowed to receive outbound CMAEG requests.
    pub providers: ProviderRegistry,
    /// TCP/TLS connection establishment timeout for provider clients.
    pub provider_connect_timeout: std::time::Duration,
    /// Provider request timeout for non-streaming setup calls.
    pub provider_request_timeout: std::time::Duration,
    /// Deployment-wide upper bound applied to each provider semaphore.
    pub provider_max_inflight: usize,
    /// Configuration required to expose the channel management API.
    pub management: ManagementConfig,
    pub cma_service_account: Option<ServiceAccountConfig>,
    pub channel_cache: ChannelCacheConfig,
    pub ingress: IngressConfig,
    pub turns: TurnConfig,
    pub delivery: DeliveryConfig,
    pub media: MediaConfig,
    pub stt: SttConfig,
    pub transformer: TransformerConfig,
}

/// Secrets and origins used by the channel management plane.
#[derive(Debug, Clone)]
pub struct ManagementConfig {
    pub public_url: String,
    pub master_keys: Arc<MasterKeyRing>,
    pub cma_base_url: String,
    pub cma_service_token: Secret,
    pub nyxid_base_url: String,
    pub tools_nyxid_slug: String,
}

impl Config {
    /// Read configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let value = env::var(CMAEG_LISTEN).unwrap_or_else(|_| "0.0.0.0:8090".to_owned());
        let listen = value.parse().map_err(|source| ConfigError::InvalidListen {
            name: CMAEG_LISTEN,
            value,
            source,
        })?;
        let value = env::var(CMAEG_METRICS_LISTEN).unwrap_or_else(|_| "0.0.0.0:9091".to_owned());
        let metrics_listen = value.parse().map_err(|source| ConfigError::InvalidListen {
            name: CMAEG_METRICS_LISTEN,
            value,
            source,
        })?;
        let providers_json = env::var(CMAEG_PROVIDERS).unwrap_or_else(|_| "[]".to_owned());
        let providers = ProviderRegistry::parse(&providers_json)?;
        // Validated against the registry, so it can only follow it.
        let media = media::config_from_env(&providers)?;
        let stt = stt::config_from_env()?;
        let transformer = transformer::config_from_env()?;
        transformer.check(&media, &stt)?;
        let provider_connect_timeout = positive_duration(CMAEG_PROVIDER_CONNECT_TIMEOUT_MS, 2_000)?;
        let provider_request_timeout =
            positive_duration(CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, 30_000)?;
        let provider_max_inflight = env::var(CMAEG_PROVIDER_MAX_INFLIGHT)
            .unwrap_or_else(|_| "256".to_owned())
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| ConfigError::InvalidMaxInflight {
                value: env::var(CMAEG_PROVIDER_MAX_INFLIGHT).unwrap_or_else(|_| "256".to_owned()),
            })?;
        let management = ManagementConfig::from_env()?;
        let turns = TurnConfig::from_env()?;
        media.check_turn_timeout(turns.turn_timeout)?;
        Ok(Self {
            listen,
            metrics_listen,
            providers,
            provider_connect_timeout,
            provider_request_timeout,
            provider_max_inflight,
            management,
            cma_service_account: ServiceAccountConfig::from_env()?,
            channel_cache: ChannelCacheConfig::from_env()?,
            ingress: IngressConfig::from_env()?,
            turns,
            delivery: DeliveryConfig::from_env()?,
            media,
            stt,
            transformer,
        })
    }
}

impl ManagementConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let public_url = required_origin(CMAEG_PUBLIC_URL)?;
        let cma_base_url = required_origin(CMAEG_CMA_BASE_URL)?;
        let nyxid_base_url = required_origin(CMAEG_NYXID_BASE_URL)?;
        let key_spec = secrets::read(CMAEG_MASTER_KEYS, CMAEG_MASTER_KEYS_FILE)?;
        let master_keys =
            MasterKeyRing::parse(&key_spec).map_err(|error| ConfigError::InvalidManagement {
                detail: format!("{CMAEG_MASTER_KEYS} is not a valid key ring: {error}"),
            })?;
        let cma_service_token =
            secrets::read(CMAEG_CMA_SERVICE_TOKEN, CMAEG_CMA_SERVICE_TOKEN_FILE)?;
        if cma_service_token.len() > 16_384
            || !cma_service_token
                .bytes()
                .all(|byte| (33..=126).contains(&byte))
        {
            return Err(ConfigError::InvalidManagement {
                detail: format!("{CMAEG_CMA_SERVICE_TOKEN} must contain a printable bearer token"),
            });
        }
        let tools_nyxid_slug =
            env::var(CMAEG_TOOLS_NYXID_SLUG).unwrap_or_else(|_| "cmaeg".to_owned());
        if !(1..=32).contains(&tools_nyxid_slug.len())
            || !tools_nyxid_slug
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ConfigError::InvalidManagement {
                detail: format!("{CMAEG_TOOLS_NYXID_SLUG} must match [a-z0-9_]{{1,32}}"),
            });
        }
        Ok(Self {
            public_url,
            master_keys: Arc::new(master_keys),
            cma_base_url,
            cma_service_token: Secret::new(cma_service_token),
            nyxid_base_url,
            tools_nyxid_slug,
        })
    }
}

fn required_origin(name: &'static str) -> Result<String, ConfigError> {
    let value = env::var(name).map_err(|_| ConfigError::InvalidManagement {
        detail: format!("{name} must be set"),
    })?;
    validate_origin(name, &value)
}

pub(crate) fn validate_origin(name: &'static str, value: &str) -> Result<String, ConfigError> {
    let url = Url::parse(value).map_err(|error| ConfigError::InvalidManagement {
        detail: format!("{name} is not an origin URL: {error}"),
    })?;
    if url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "" && url.path() != "/")
        || value.ends_with('/')
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!(
                "{name} must be an origin without credentials, path, query, or trailing slash"
            ),
        });
    }
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && url.host().is_some_and(|host| match host {
                url::Host::Domain(domain) => {
                    domain.eq_ignore_ascii_case("localhost")
                        || (name != CMAEG_PUBLIC_URL && domain.ends_with(".svc"))
                }
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
            }))
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must use https (or loopback http for local tests)"),
        });
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn positive_duration(
    name: &'static str,
    default_ms: u64,
) -> Result<std::time::Duration, ConfigError> {
    let value = env::var(name).unwrap_or_else(|_| default_ms.to_string());
    let milliseconds = value
        .parse::<u64>()
        .ok()
        .filter(|value| {
            *value > 0
                && std::time::Instant::now()
                    .checked_add(std::time::Duration::from_millis(*value))
                    .is_some()
        })
        .ok_or_else(|| ConfigError::InvalidDuration {
            name,
            value: value.clone(),
        })?;
    Ok(std::time::Duration::from_millis(milliseconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_defaults_are_positive() {
        assert_eq!(
            positive_duration("__missing_cmaeg_key__", 2_000)
                .unwrap()
                .as_millis(),
            2_000
        );
    }
    #[test]
    fn origins_keep_public_callbacks_strict_and_allow_configured_cluster_services() {
        for value in [
            "https://gateway.example/",
            "https://u:p@gateway.example",
            "https://gateway.example/path",
            "https://gateway.example?q=1",
            "http://public.example",
            "file:///tmp/fixture",
        ] {
            assert!(validate_origin(CMAEG_PUBLIC_URL, value).is_err());
        }
        for value in [
            "http://127.0.0.1:8090",
            "http://[::1]:8090",
            "https://gateway.example",
        ] {
            assert!(validate_origin(CMAEG_PUBLIC_URL, value).is_ok());
        }
        assert!(validate_origin(CMAEG_CMA_BASE_URL, "http://example-host-01.example-namespace.svc").is_ok());
        assert!(validate_origin(CMAEG_NYXID_BASE_URL, "http://nyxid.chrono-bot.svc").is_ok());
        assert!(validate_origin(CMAEG_PUBLIC_URL, "http://cmaeg.chrono-bot.svc").is_err());
        assert_eq!(
            validate_origin(CMAEG_CMA_BASE_URL, "https://CMA.EXAMPLE:443").unwrap(),
            "https://cma.example"
        );
    }
}
