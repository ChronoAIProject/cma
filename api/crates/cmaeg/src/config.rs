//! Environment configuration for the CMA Event Gateway.

use std::{env, fs, net::SocketAddr, path::Path, sync::Arc};

use crate::providers::ProviderRegistry;
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use reqwest::Url;

/// The listener address used by CMAEG.
pub const CMAEG_LISTEN: &str = "CMAEG_LISTEN";
pub const CMAEG_PROVIDERS: &str = "CMAEG_PROVIDERS";
pub const CMAEG_PROVIDER_CONNECT_TIMEOUT_MS: &str = "CMAEG_PROVIDER_CONNECT_TIMEOUT_MS";
pub const CMAEG_PROVIDER_REQUEST_TIMEOUT_MS: &str = "CMAEG_PROVIDER_REQUEST_TIMEOUT_MS";
pub const CMAEG_PROVIDER_MAX_INFLIGHT: &str = "CMAEG_PROVIDER_MAX_INFLIGHT";
pub const CMAEG_PUBLIC_URL: &str = "CMAEG_PUBLIC_URL";
pub const CMAEG_MASTER_KEYS_FILE: &str = "CMAEG_MASTER_KEYS_FILE";
pub const CMAEG_CMA_BASE_URL: &str = "CMAEG_CMA_BASE_URL";
pub const CMAEG_CMA_SERVICE_TOKEN_FILE: &str = "CMAEG_CMA_SERVICE_TOKEN_FILE";
pub const CMAEG_NYXID_BASE_URL: &str = "CMAEG_NYXID_BASE_URL";
pub const CMAEG_TOOLS_NYXID_SLUG: &str = "CMAEG_TOOLS_NYXID_SLUG";

/// Configuration errors that prevent the service from binding safely.
#[derive(Debug)]
pub enum ConfigError {
    /// The configured listener was not a valid socket address.
    InvalidListen {
        value: String,
        source: std::net::AddrParseError,
    },
    /// The provider registry JSON did not satisfy the outbound-call policy.
    InvalidProviders { detail: String },
    /// A duration setting was not a positive bounded integer.
    InvalidDuration { name: &'static str, value: String },
    /// The global provider concurrency cap was invalid.
    InvalidMaxInflight { value: String },
    /// A management-plane origin, mounted secret, or slug was invalid.
    InvalidManagement { detail: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidListen { value, source } => {
                write!(formatter, "{CMAEG_LISTEN}={value:?} is invalid: {source}")
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
        }
    }
}

impl std::error::Error for ConfigError {}

/// Process configuration loaded once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address on which the HTTP service accepts requests.
    pub listen: SocketAddr,
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
        let listen = value
            .parse()
            .map_err(|source| ConfigError::InvalidListen { value, source })?;
        let providers_json = env::var(CMAEG_PROVIDERS).unwrap_or_else(|_| "[]".to_owned());
        let providers = ProviderRegistry::parse(&providers_json)?;
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
        Ok(Self {
            listen,
            providers,
            provider_connect_timeout,
            provider_request_timeout,
            provider_max_inflight,
            management,
        })
    }
}

impl ManagementConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let public_url = required_origin(CMAEG_PUBLIC_URL)?;
        let cma_base_url = required_origin(CMAEG_CMA_BASE_URL)?;
        let nyxid_base_url = required_origin(CMAEG_NYXID_BASE_URL)?;
        let key_file = required_path(CMAEG_MASTER_KEYS_FILE)?;
        let key_spec = read_first_line(&key_file, CMAEG_MASTER_KEYS_FILE)?;
        let master_keys =
            MasterKeyRing::parse(&key_spec).map_err(|error| ConfigError::InvalidManagement {
                detail: format!("{CMAEG_MASTER_KEYS_FILE} is not a valid key ring: {error}"),
            })?;
        let token_file = required_path(CMAEG_CMA_SERVICE_TOKEN_FILE)?;
        let cma_service_token = read_secret_file(&token_file, CMAEG_CMA_SERVICE_TOKEN_FILE)?;
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

fn required_path(name: &'static str) -> Result<String, ConfigError> {
    let value = env::var(name).map_err(|_| ConfigError::InvalidManagement {
        detail: format!("{name} must be set"),
    })?;
    if value.is_empty() {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must not be empty"),
        });
    }
    Ok(value)
}

fn required_origin(name: &'static str) -> Result<String, ConfigError> {
    let value = env::var(name).map_err(|_| ConfigError::InvalidManagement {
        detail: format!("{name} must be set"),
    })?;
    validate_origin(name, &value)
}

fn validate_origin(name: &'static str, value: &str) -> Result<String, ConfigError> {
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

fn read_first_line(path: &str, name: &'static str) -> Result<String, ConfigError> {
    let content =
        fs::read_to_string(Path::new(path)).map_err(|error| ConfigError::InvalidManagement {
            detail: format!("cannot read {name}: {error}"),
        })?;
    let line = content.trim().to_owned();
    if line.is_empty() || line.lines().count() != 1 {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must contain one non-empty line"),
        });
    }
    Ok(line)
}

fn read_secret_file(path: &str, name: &'static str) -> Result<String, ConfigError> {
    let content =
        fs::read_to_string(Path::new(path)).map_err(|error| ConfigError::InvalidManagement {
            detail: format!("cannot read {name}: {error}"),
        })?;
    let value = content.trim().to_owned();
    if value.is_empty()
        || value.len() > 16_384
        || !value.bytes().all(|byte| (33..=126).contains(&byte))
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must contain a non-empty printable bearer token"),
        });
    }
    Ok(value)
}

fn positive_duration(
    name: &'static str,
    default_ms: u64,
) -> Result<std::time::Duration, ConfigError> {
    let value = env::var(name).unwrap_or_else(|_| default_ms.to_string());
    let milliseconds = value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
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
