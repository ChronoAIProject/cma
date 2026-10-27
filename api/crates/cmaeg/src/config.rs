//! Environment configuration for the CMA Event Gateway.

use std::{env, net::SocketAddr, sync::Arc};

use crate::providers::ProviderRegistry;
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use reqwest::Url;

mod cache;
mod console;
mod delivery;
mod duration;
use duration::positive_duration;
mod ingress;
mod media;
mod native_discord;
mod routing;
mod secrets;
mod service;
mod service_account;
pub use service::*;
mod stt;
pub mod transformer;
mod turns;
pub use cache::*;
pub use console::*;
pub use delivery::*;
pub use ingress::*;
pub use media::{
    MediaConfig, MediaToolProviders, CMAEG_MEDIA_CACHE_BYTES, CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR,
    CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_INFLIGHT_BYTES, CMAEG_MEDIA_LINK_TTL_SECONDS,
    CMAEG_MEDIA_MAX_BYTES, CMAEG_MEDIA_MAX_JOBS, CMAEG_MEDIA_PREPARE_BUDGET_MS,
    CMAEG_MEDIA_RATE_PER_MINUTE, CMAEG_MEDIA_RESPONSE_BYTES, CMAEG_MEDIA_TOOL_PROVIDERS,
};
pub use native_discord::NativeDiscordConfig;
pub use routing::{RoutingConfig, CMAEG_NON_HUMAN_PROVIDERS};
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
pub const CMAEG_PUBLIC_API_URL: &str = "CMAEG_PUBLIC_API_URL";
pub const CMAEG_ADVERTISE_PUBLIC_API_URL: &str = "CMAEG_ADVERTISE_PUBLIC_API_URL";
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
    /// A console setting was invalid.
    InvalidConsole { detail: String },
    /// A multi-consumer routing setting was invalid.
    Routing(String),
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
            Self::InvalidConsole { detail } => {
                write!(formatter, "console configuration is invalid: {detail}")
            }
            Self::Routing(detail) => {
                write!(formatter, "routing configuration is invalid: {detail}")
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
    pub native_bot_mentions_writes_enabled: bool,
    /// Allows new thread contracts only after every reader supports them.
    pub thread_contract_writes_enabled: bool,
    pub multi_consumer_writes_enabled: bool,
    pub dynamic_sources_writes_enabled: bool,
    pub relay_raw_verification_writes_enabled: bool,
    pub lark_resource_writes_enabled: bool,
    pub native_lark_writes_enabled: bool,
    pub native_lark_intake_enabled: bool,
    pub native_lark_media_enabled: bool,
    pub native_whatsapp_groups_writes_enabled: bool,
    pub native_whatsapp_groups_enabled: bool,
    pub native_lark_message_effects_enabled: bool,
    pub native_lark_received_reactions_enabled: bool,
    pub native_whatsapp_writes_enabled: bool,
    pub native_whatsapp_media_enabled: bool,
    pub native_lark_outbound_media_enabled: bool,
    pub native_whatsapp_outbound_media_enabled: bool,
    pub native_whatsapp_interactions_enabled: bool,
    pub native_whatsapp_reactions_enabled: bool,
    pub native_telegram_received_reactions_enabled: bool,
    pub native_discord_received_reactions_enabled: bool,
    pub native_whatsapp_templates_enabled: bool,
    pub lark_message_effects_writes_enabled: bool,
    pub native_discord_writes_enabled: bool,
    pub native_telegram_message_events_writes_enabled: bool,
    pub native_discord_thread_history_enabled: bool,
    pub native_discord_reactions_enabled: bool,
    pub native_discord: NativeDiscordConfig,
    pub outbound_messages_writes_enabled: bool,
    pub ingress: IngressConfig,
    pub turns: TurnConfig,
    pub delivery: DeliveryConfig,
    pub media: MediaConfig,
    pub routing: RoutingConfig,
    pub stt: SttConfig,
    pub transformer: TransformerConfig,
    pub console: ConsoleConfig,
}

/// Secrets and origins used by the channel management plane.
#[derive(Debug, Clone)]
pub struct ManagementConfig {
    /// Legacy browser console authority; remains an origin.
    pub public_url: String,
    /// Canonical externally reachable API base, optionally with a path prefix.
    pub public_api_url: String,
    /// Disable only while old replicas still lack the canonical browser routes.
    pub advertise_public_api_url: bool,
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
        let routing = routing::config_from_env(&providers)?;
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
            native_bot_mentions_writes_enabled: match env::var(
                "CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED",
            ) {
                Err(env::VarError::NotPresent) => false,
                Ok(value) if value == "false" => false,
                Ok(value) if value == "true" => true,
                _ => {
                    return Err(ConfigError::InvalidManagement {
                        detail: "CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED must be true or false"
                            .into(),
                    })
                }
            },
            thread_contract_writes_enabled: match env::var("CMAEG_THREAD_CONTRACT_WRITES_ENABLED") {
                Err(env::VarError::NotPresent) => false,
                Ok(value) if value == "false" => false,
                Ok(value) if value == "true" => true,
                _ => {
                    return Err(ConfigError::InvalidManagement {
                        detail: "CMAEG_THREAD_CONTRACT_WRITES_ENABLED must be true or false".into(),
                    })
                }
            },
            multi_consumer_writes_enabled: routing::write_flag(
                "CMAEG_MULTI_CONSUMER_WRITES_ENABLED",
            )?,
            dynamic_sources_writes_enabled: routing::write_flag(
                "CMAEG_DYNAMIC_SOURCES_WRITES_ENABLED",
            )?,
            native_discord_reactions_enabled: routing::write_flag(
                "CMAEG_DISCORD_REACTIONS_ENABLED",
            )?,
            native_discord_thread_history_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_THREAD_HISTORY_ENABLED",
            )?,
            native_telegram_message_events_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_TELEGRAM_MESSAGE_EVENTS_WRITES_ENABLED",
            )?,
            native_lark_writes_enabled: routing::write_flag("CMAEG_NATIVE_LARK_WRITES_ENABLED")?,
            native_lark_intake_enabled: routing::write_flag("CMAEG_NATIVE_LARK_INTAKE_ENABLED")?,
            native_lark_media_enabled: routing::write_flag("CMAEG_NATIVE_LARK_MEDIA_ENABLED")?,
            native_whatsapp_groups_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED",
            )?,
            native_whatsapp_groups_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED",
            )?,
            native_lark_message_effects_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED",
            )?,
            native_lark_received_reactions_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED",
            )?,
            native_whatsapp_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED",
            )?,
            native_lark_outbound_media_enabled: routing::write_flag(
                "CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED",
            )?,
            native_whatsapp_outbound_media_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED",
            )?,
            native_whatsapp_interactions_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED",
            )?,
            native_whatsapp_reactions_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED",
            )?,
            native_telegram_received_reactions_enabled: routing::telegram_received_reactions()?,
            native_discord_received_reactions_enabled: routing::discord_received_reactions()?,
            native_whatsapp_templates_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED",
            )?,
            native_whatsapp_media_enabled: routing::write_flag(
                "CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED",
            )?,
            native_discord_writes_enabled: routing::write_flag(
                "CMAEG_NATIVE_DISCORD_WRITES_ENABLED",
            )?,
            relay_raw_verification_writes_enabled: routing::write_flag(
                "CMAEG_RELAY_RAW_VERIFICATION_WRITES_ENABLED",
            )?,
            lark_message_effects_writes_enabled: routing::write_flag(
                "CMAEG_LARK_MESSAGE_EFFECTS_WRITES_ENABLED",
            )?,
            lark_resource_writes_enabled: routing::write_flag(
                "CMAEG_LARK_RESOURCE_WRITES_ENABLED",
            )?,
            outbound_messages_writes_enabled: routing::write_flag(
                "CMAEG_OUTBOUND_MESSAGES_WRITES_ENABLED",
            )?,
            ingress: IngressConfig::from_env()?,
            turns,
            delivery: DeliveryConfig::from_env()?,
            media,
            native_discord: native_discord::config_from_env(&routing)?,
            routing,
            stt,
            transformer,
            console: ConsoleConfig::from_env()?,
        })
    }
}

impl ManagementConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let public_url = required_origin(CMAEG_PUBLIC_URL)?;
        let public_api_url = env::var(CMAEG_PUBLIC_API_URL)
            .map(|value| validate_public_api_url(&value))
            .unwrap_or_else(|_| Ok(public_url.clone()))?;
        let advertisement = env::var(CMAEG_ADVERTISE_PUBLIC_API_URL);
        let advertise_public_api_url = parse_api_advertisement(match &advertisement {
            Ok(value) => Some(value.as_str()),
            Err(env::VarError::NotPresent) => None,
            Err(_) => Some("invalid"),
        })?;
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
            public_api_url,
            advertise_public_api_url,
            master_keys: Arc::new(master_keys),
            cma_base_url,
            cma_service_token: Secret::new(cma_service_token),
            nyxid_base_url,
            tools_nyxid_slug,
        })
    }
}

fn parse_api_advertisement(value: Option<&str>) -> Result<bool, ConfigError> {
    match value {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_ADVERTISE_PUBLIC_API_URL} must be true or false"),
        }),
    }
}

/// A trusted base used for generated links and routing, never taken from Host.
fn validate_public_api_url(value: &str) -> Result<String, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidManagement {
        detail: format!("{CMAEG_PUBLIC_API_URL} must be a public API base URL"),
    })?;
    validate_origin(CMAEG_PUBLIC_URL, &url.origin().ascii_serialization())?;
    let path = url.path();
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.ends_with('/')
        || value.len() > 2048
        || value != url.as_str().trim_end_matches('/')
        || (path != "/"
            && path
                .strip_prefix('/')
                .unwrap_or_default()
                .split('/')
                .any(|part| {
                    part.is_empty()
                        || !part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                }))
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_PUBLIC_API_URL} must be a canonical HTTPS base with clean path segments and no credentials, query, fragment or trailing slash"),
        });
    }
    Ok(value.to_owned())
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod public_api_tests;
