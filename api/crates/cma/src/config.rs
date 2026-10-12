//! Deployment configuration retained across the resource-model rebuild.
//!
//! Process transport, authentication, durable infrastructure and protection.

use std::{collections::BTreeSet, net::SocketAddr, num::NonZeroU32, time::Duration};

use axum::http::HeaderValue;
use ipnet::IpNet;
use layer2_core::ServiceId;

use crate::{auth::AuthMode, store::AuthEncryptionKey};

mod agent_sandbox;
pub(crate) mod bridge_flags;
mod execution;
mod gateway;
mod home;
mod profiles;
mod runtime;
mod storage;

pub(crate) use bridge_flags::BridgeFeatureFlags;
pub(crate) use gateway::GatewayConfig;
pub(crate) use profiles::DefaultProfileConfig;
pub(crate) use runtime::RuntimeConfig;
pub(crate) use storage::CheckpointStorageConfig;

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_PUBLIC_URL: &str = "http://localhost:8080";
const DEFAULT_CORS_ORIGIN: &str = "http://localhost:5173";
const DEFAULT_DATABASE: &str = "chrono_bot";
const DEFAULT_ACTIVITY_RETENTION_DAYS: u64 = 90;
const DEFAULT_BASELINE_SERVICES: &str = "chrono-llm-public,ornn-api";
const DEFAULT_TRUSTED_PROXY_HOPS: usize = 1;
const MAX_TRUSTED_PROXY_HOPS: usize = 16;
const MINIMUM_HEALTH_REQUESTS_PER_MINUTE: u32 = 30;
const PINNED_CLOUDFLARE_PROXY_CIDRS: &str = concat!(
    "173.245.48.0/20,103.21.244.0/22,103.22.200.0/22,103.31.4.0/22,",
    "141.101.64.0/18,108.162.192.0/18,190.93.240.0/20,188.114.96.0/20,",
    "197.234.240.0/22,198.41.128.0/17,162.158.0.0/15,104.16.0.0/13,",
    "104.24.0.0/14,172.64.0.0/13,131.0.72.0/22,2400:cb00::/32,",
    "2606:4700::/32,2803:f800::/32,2405:b500::/32,2405:8100::/32,",
    "2a06:98c0::/29,2c0f:f248::/32",
);

/// Per-key request budgets enforced by this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub admission_per_minute: NonZeroU32,
    pub health_per_minute: NonZeroU32,
    pub default_per_minute: NonZeroU32,
    pub create_per_minute: NonZeroU32,
    pub input_per_minute: NonZeroU32,
}

impl RateLimitConfig {
    fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        Ok(Self {
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
        })
    }
}

/// TCP peers allowed to provide the load balancer's client-address chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxyConfig {
    pub cidrs: Vec<IpNet>,
    pub forwarded_hops: usize,
    pub cloudflare_cidrs: Vec<IpNet>,
}

impl TrustedProxyConfig {
    fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let cidrs = parse_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            &value_or(lookup, "CHRONO_SERVER_TRUSTED_PROXY_CIDRS", "")?,
        )?;
        let cloudflare_cidrs = parse_networks(
            "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS",
            &value_or(lookup, "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS", "")?,
        )?;
        validate_cloudflare_networks(&cidrs, &cloudflare_cidrs)?;
        let forwarded_hops = parse_usize(
            "CHRONO_SERVER_TRUSTED_PROXY_HOPS",
            &value_or(
                lookup,
                "CHRONO_SERVER_TRUSTED_PROXY_HOPS",
                &DEFAULT_TRUSTED_PROXY_HOPS.to_string(),
            )?,
            1,
            MAX_TRUSTED_PROXY_HOPS,
        )?;
        Ok(Self {
            cidrs,
            forwarded_hops,
            cloudflare_cidrs,
        })
    }

    #[cfg(test)]
    pub(crate) fn sample() -> Self {
        Self {
            cidrs: Vec::new(),
            forwarded_hops: DEFAULT_TRUSTED_PROXY_HOPS,
            cloudflare_cidrs: Vec::new(),
        }
    }
}

/// Complete process configuration for the temporary discovery boundary.
#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub public_url: String,
    pub nyxid_public_url: Option<String>,
    pub cors_origins: Vec<HeaderValue>,
    pub rate_limits: RateLimitConfig,
    pub trusted_proxies: TrustedProxyConfig,
    pub mongodb_uri: String,
    pub mongodb_database: String,
    pub deployment_id: String,
    pub runtime: RuntimeConfig,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub checkpoint_storage: CheckpointStorageConfig,
    pub baseline_services: Vec<ServiceId>,
    pub auth_encryption_key: AuthEncryptionKey,
    pub activity_retention: Duration,
    pub default_profiles: DefaultProfileConfig,
    pub auth_mode: AuthMode,
    pub bridge_flags: BridgeFeatureFlags,
    pub gateway: Option<GatewayConfig>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("nyxid_public_url", &self.nyxid_public_url)
            .field("cors_origins", &self.cors_origins)
            .field("rate_limits", &self.rate_limits)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("mongodb_uri", &"<redacted>")
            .field("mongodb_database", &self.mongodb_database)
            .field("deployment_id", &self.deployment_id)
            .field("runtime", &self.runtime)
            .field("execution", &self.execution)
            .field("checkpoint_storage", &self.checkpoint_storage)
            .field("baseline_services", &self.baseline_services)
            .field("auth_encryption_key", &self.auth_encryption_key)
            .field("activity_retention", &self.activity_retention)
            .field("default_profiles", &self.default_profiles)
            .field("auth_mode", &self.auth_mode)
            .field("bridge_flags", &self.bridge_flags)
            .finish()
    }
}

impl Config {
    /// Read and validate configuration exactly once at process startup.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(strict_env)
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let bind = value_or(&mut lookup, "CHRONO_SERVER_BIND", DEFAULT_BIND)?
            .parse::<SocketAddr>()
            .map_err(|error| format!("CHRONO_SERVER_BIND is not a socket address: {error}"))?;
        let public_url = value_or(&mut lookup, "CHRONO_SERVER_PUBLIC_URL", DEFAULT_PUBLIC_URL)?;
        validate_origin("CHRONO_SERVER_PUBLIC_URL", &public_url)?;
        let cors_origins = value_or(
            &mut lookup,
            "CHRONO_SERVER_CORS_ORIGIN",
            DEFAULT_CORS_ORIGIN,
        )?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            validate_origin("CHRONO_SERVER_CORS_ORIGIN", value)?;
            HeaderValue::from_str(value).map_err(|error| {
                format!("CHRONO_SERVER_CORS_ORIGIN contains an invalid header value: {error}")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
        if cors_origins.is_empty() {
            return Err("CHRONO_SERVER_CORS_ORIGIN must contain at least one origin".into());
        }

        let bridge_flags = BridgeFeatureFlags::from_lookup(&mut lookup)?;
        let rate_limits = RateLimitConfig::from_lookup(&mut lookup)?;
        let trusted_proxies = TrustedProxyConfig::from_lookup(&mut lookup)?;
        let mongodb_uri = required(&mut lookup, "CHRONO_MONGODB_URI")?;
        validate_mongodb_uri(&mongodb_uri)?;
        let mongodb_database = value_or(&mut lookup, "CHRONO_MONGODB_DATABASE", DEFAULT_DATABASE)?;
        if mongodb_database.is_empty()
            || mongodb_database.len() > 63
            || mongodb_database
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || b"/\\.\"$".contains(&byte))
        {
            return Err("CHRONO_MONGODB_DATABASE is not a valid MongoDB database name".into());
        }

        let deployment_id = runtime::deployment_id_from_lookup(&mut lookup)?;
        let runtime = RuntimeConfig::from_lookup(&mut lookup)?;
        let execution = execution::from_lookup(&mut lookup)?;
        let checkpoint_storage = CheckpointStorageConfig::from_lookup(&mut lookup)?;
        let baseline_services = parse_service_ids(
            "CHRONO_BASELINE_SERVICES",
            &value_or(
                &mut lookup,
                "CHRONO_BASELINE_SERVICES",
                DEFAULT_BASELINE_SERVICES,
            )?,
        )?;
        let auth_encryption_key =
            AuthEncryptionKey::from_base64(&required(&mut lookup, "CHRONO_AUTH_ENCRYPTION_KEY")?)?;
        let activity_retention_days = parse_u64(
            "CHRONO_ACTIVITY_RETENTION_DAYS",
            &value_or(
                &mut lookup,
                "CHRONO_ACTIVITY_RETENTION_DAYS",
                &DEFAULT_ACTIVITY_RETENTION_DAYS.to_string(),
            )?,
            1,
            3_650,
        )?;
        let activity_retention = Duration::from_secs(
            activity_retention_days
                .checked_mul(86_400)
                .expect("the validated day count fits in seconds"),
        );
        let default_profiles = DefaultProfileConfig::from_lookup(&mut lookup)?;

        let (auth_mode, nyxid_public_url) =
            match value_or(&mut lookup, "CHRONO_AUTH_MODE", "provider")?.as_str() {
                "trusted" => (
                    AuthMode::Trusted {
                        subject: value_or(
                            &mut lookup,
                            "CHRONO_TRUSTED_SUBJECT",
                            "local-developer",
                        )?,
                    },
                    None,
                ),
                "provider" => (
                    AuthMode::Provider {
                        base_url: required(&mut lookup, "CHRONO_NYXID_BASE_URL")?,
                        client_id: required(&mut lookup, "CHRONO_NYXID_CLIENT_ID")?,
                        client_secret: required(&mut lookup, "CHRONO_NYXID_CLIENT_SECRET")?,
                        public_url: public_url.clone(),
                    },
                    Some(parse_canonical_https_origin(
                        "CHRONO_NYXID_PUBLIC_URL",
                        &required(&mut lookup, "CHRONO_NYXID_PUBLIC_URL")?,
                    )?),
                ),
                value => {
                    return Err(format!(
                        "CHRONO_AUTH_MODE must be \"provider\" or \"trusted\", got {value:?}"
                    ))
                }
            };
        if let AuthMode::Provider { base_url, .. } = &auth_mode {
            validate_origin("CHRONO_NYXID_BASE_URL", base_url)?;
        }

        Ok(Self {
            bind,
            public_url,
            nyxid_public_url,
            cors_origins,
            rate_limits,
            trusted_proxies,
            mongodb_uri,
            mongodb_database,
            deployment_id,
            runtime,
            execution,
            checkpoint_storage,
            baseline_services,
            auth_encryption_key,
            activity_retention,
            default_profiles,
            auth_mode,
            bridge_flags,
            gateway: GatewayConfig::from_lookup(&mut lookup)?,
        })
    }

    pub fn auth_mode_name(&self) -> &'static str {
        match self.auth_mode {
            AuthMode::Provider { .. } => "provider",
            AuthMode::Trusted { .. } => "trusted",
        }
    }
}

fn strict_env(key: &str) -> Result<Option<String>, String> {
    std::env::var(key).map(Some).or_else(|error| match error {
        std::env::VarError::NotPresent => Ok(None),
        std::env::VarError::NotUnicode(_) => Err(format!("{key} is not valid Unicode")),
    })
}

fn value_or(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    Ok(lookup(key)?.unwrap_or_else(|| default.to_owned()))
}

fn required(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
) -> Result<String, String> {
    lookup(key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn positive_u32(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: u32,
    minimum: u32,
) -> Result<NonZeroU32, String> {
    let value = parse_u64(
        key,
        &value_or(lookup, key, &default.to_string())?,
        u64::from(minimum),
        u64::from(u32::MAX),
    )? as u32;
    NonZeroU32::new(value).ok_or_else(|| format!("{key} must be positive"))
}

fn parse_u64(key: &str, raw: &str, minimum: u64, maximum: u64) -> Result<u64, String> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| format!("{key} must be an unsigned integer"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

fn parse_usize(key: &str, raw: &str, minimum: usize, maximum: usize) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("{key} must be an unsigned integer"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

fn parse_networks(key: &str, raw: &str) -> Result<Vec<IpNet>, String> {
    let mut networks = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<IpNet>()
                .map_err(|error| format!("{key} contains invalid network {value:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    networks.sort_by_key(ToString::to_string);
    networks.dedup();
    Ok(networks)
}

fn parse_service_ids(key: &str, raw: &str) -> Result<Vec<ServiceId>, String> {
    let values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                Err(format!("{key} contains invalid service {value:?}"))
            } else {
                Ok(value.to_owned())
            }
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if values.is_empty() || values.len() > 64 {
        return Err(format!("{key} must contain between 1 and 64 services"));
    }
    Ok(values.into_iter().map(ServiceId::new).collect())
}

fn validate_cloudflare_networks(gfe: &[IpNet], cloudflare: &[IpNet]) -> Result<(), String> {
    if !cloudflare.is_empty() {
        let pinned = parse_networks(
            "build-pinned Cloudflare networks",
            PINNED_CLOUDFLARE_PROXY_CIDRS,
        )
        .expect("the build-pinned Cloudflare networks are valid");
        if cloudflare != pinned {
            return Err(
                "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS must be empty or match the build-pinned set"
                    .into(),
            );
        }
    }
    if gfe.iter().any(|left| {
        cloudflare
            .iter()
            .any(|right| left.contains(&right.network()) || right.contains(&left.network()))
    }) {
        return Err("trusted proxy and Cloudflare networks must not overlap".into());
    }
    Ok(())
}

fn validate_origin(key: &str, value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|error| format!("{key} is not a URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(format!(
            "{key} must be an HTTP(S) origin without credentials or a path"
        ));
    }
    Ok(())
}

fn parse_canonical_https_origin(key: &str, raw: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|_| format!("{key} must be one canonical absolute HTTPS origin"))?;
    let canonical = parsed.origin().ascii_serialization();
    if raw.len() > 2_048
        || parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw != canonical
    {
        return Err(format!("{key} must be one canonical absolute HTTPS origin"));
    }
    Ok(canonical)
}

fn validate_mongodb_uri(value: &str) -> Result<(), String> {
    if value.starts_with("mongodb://") || value.starts_with("mongodb+srv://") {
        Ok(())
    } else {
        Err("CHRONO_MONGODB_URI must use mongodb:// or mongodb+srv://".into())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "config/home_tests.rs"]
mod home_tests;
