//! Deployment configuration retained across the resource-model rebuild.
//!
//! Process transport, authentication, durable infrastructure and protection.

use std::{collections::BTreeSet, net::SocketAddr, num::NonZeroU32, time::Duration};

use axum::http::HeaderValue;
use ipnet::IpNet;
use layer2_core::ServiceId;

use crate::{auth::AuthMode, store::AuthEncryptionKey};

mod agent_sandbox;
mod execution;
mod gateway;
mod home;
mod profiles;
mod runtime;

mod features;
mod task_experience;
pub(crate) use features::FeatureFlags;
pub(crate) use gateway::GatewayConfig;
mod bot_father;
mod documents;
pub(crate) use bot_father::BotFatherConfig;
pub(crate) use documents::DocumentStorageConfig;
pub(crate) use profiles::DefaultProfileConfig;
pub(crate) use runtime::RuntimeConfig;

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_PUBLIC_URL: &str = "http://localhost:8080";
const DEFAULT_CORS_ORIGIN: &str = "http://localhost:5173";
const DEFAULT_DATABASE: &str = "chrono_bot";
const DEFAULT_ACTIVITY_RETENTION_DAYS: u64 = 90;
pub(crate) const REQUIRED_BASELINE_SERVICES: &[&str] = &["chrono-llm-public", "ornn-api"];
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
    /// Base URL for the independently deployed Image Manager service.  CMA
    /// only uses it with the request's delegated user credential.
    pub image_manager_base_url: String,
    pub nyxid_public_url: Option<String>,
    pub nyxid_service_id: Option<String>,
    pub nyxid_proxy_issuer: Option<String>,
    pub nyxid_proxy_jwks_url: Option<String>,
    pub cors_origins: Vec<HeaderValue>,
    pub rate_limits: RateLimitConfig,
    pub trusted_proxies: TrustedProxyConfig,
    pub mongodb_uri: String,
    pub mongodb_database: String,
    pub mongodb_authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    pub deployment_id: String,
    pub runtime: RuntimeConfig,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub baseline_services: Vec<ServiceId>,
    pub auth_encryption_key: AuthEncryptionKey,
    pub activity_retention: Duration,
    pub default_profiles: DefaultProfileConfig,
    pub agent_manager_sandbox_capacity: layer3_adapter::SandboxCapacity,
    pub desktop_verified_runtime_image: Option<String>,
    pub auth_mode: AuthMode,
    pub features: FeatureFlags,
    pub task_experience:
        std::sync::Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    pub gateway: Option<GatewayConfig>,
    pub bot_father: Option<BotFatherConfig>,
    pub document_storage: Option<DocumentStorageConfig>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("image_manager_base_url", &self.image_manager_base_url)
            .field("nyxid_public_url", &self.nyxid_public_url)
            .field("cors_origins", &self.cors_origins)
            .field("rate_limits", &self.rate_limits)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("mongodb_uri", &"<redacted>")
            .field("mongodb_database", &self.mongodb_database)
            .field("mongodb_authority", &self.mongodb_authority)
            .field("deployment_id", &self.deployment_id)
            .field("runtime", &self.runtime)
            .field("execution", &self.execution)
            .field("baseline_services", &self.baseline_services)
            .field("auth_encryption_key", &self.auth_encryption_key)
            .field("activity_retention", &self.activity_retention)
            .field("default_profiles", &self.default_profiles)
            .field(
                "agent_manager_sandbox_capacity",
                &self.agent_manager_sandbox_capacity,
            )
            .field("auth_mode", &self.auth_mode)
            .field("features", &self.features)
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
        let image_manager_base_url = parse_service_base_url(
            "CHRONO_IMAGE_MANAGER_BASE_URL",
            &value_or(
                &mut lookup,
                "CHRONO_IMAGE_MANAGER_BASE_URL",
                &format!("{}/api/image-manager", public_url.trim_end_matches('/')),
            )?,
        )?;
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

        let features = FeatureFlags::from_lookup(&mut lookup)?;
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
        let mut baseline_services = parse_service_ids(
            "CHRONO_BASELINE_SERVICES",
            &value_or(
                &mut lookup,
                "CHRONO_BASELINE_SERVICES",
                DEFAULT_BASELINE_SERVICES,
            )?,
        )?;
        baseline_services.extend(
            REQUIRED_BASELINE_SERVICES
                .iter()
                .map(|id| ServiceId::new(*id)),
        );
        baseline_services.sort();
        baseline_services.dedup();
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
        let agent_manager_sandbox_capacity = profiles::agent_manager_capacity(&mut lookup)?;

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
            image_manager_base_url,
            nyxid_public_url,
            nyxid_service_id: lookup("CHRONO_NYXID_SERVICE_ID")?.filter(|id| !id.trim().is_empty()),
            nyxid_proxy_issuer: lookup("CHRONO_NYXID_PROXY_ISSUER")?,
            nyxid_proxy_jwks_url: lookup("CHRONO_NYXID_PROXY_JWKS_URL")?,
            cors_origins,
            rate_limits,
            trusted_proxies,
            mongodb_uri,
            mongodb_database,
            mongodb_authority: cma_service_auth::mongodb_authority::MongoAuthorityMode::parse(
                lookup("CHRONO_MONGODB_AUTHORITY_MODE")?.as_deref(),
            )
            .map_err(str::to_owned)?,
            deployment_id,
            runtime,
            execution,
            baseline_services,
            auth_encryption_key,
            activity_retention,
            default_profiles,
            agent_manager_sandbox_capacity,
            desktop_verified_runtime_image: lookup("CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE")?
                .filter(|value| !value.is_empty()),
            auth_mode,
            features,
            task_experience: task_experience::from_lookup(&mut lookup)?,
            gateway: GatewayConfig::from_lookup(&mut lookup)?,
            bot_father: BotFatherConfig::from_lookup(&mut lookup)?,
            document_storage: DocumentStorageConfig::from_lookup(&mut lookup)?,
        })
    }

    pub fn auth_mode_name(&self) -> &'static str {
        match self.auth_mode {
            AuthMode::Provider { .. } => "provider",
            AuthMode::Trusted { .. } => "trusted",
        }
    }
}

#[path = "config_validation.rs"]
mod validation;
use validation::{
    parse_canonical_https_origin, parse_networks, parse_service_base_url, parse_service_ids,
    parse_u64, parse_usize, positive_u32, required, strict_env, validate_cloudflare_networks,
    validate_mongodb_uri, validate_origin, value_or,
};

#[cfg(test)]
mod tests;
