//! Deployment configuration retained across the resource-model rebuild.
//!
//! Process transport, authentication, durable infrastructure and protection.

use std::{collections::BTreeSet, net::SocketAddr, num::NonZeroU32, time::Duration};

use axum::http::HeaderValue;
use ipnet::IpNet;
use layer2_core::ServiceId;

use crate::{auth::AuthMode, store::AuthEncryptionKey};

mod access_migration;
mod access_origins;
mod access_snapshot;
mod activity;
mod lifecycle_failure;
mod startup_schema;
pub(crate) use startup_schema::StartupSchemaMode;
mod account_deletion;
mod control_admission;
mod creation_outcomes;
mod legal_terms;
pub(crate) use control_admission::ControlAdmissionConfig;
pub(crate) use creation_outcomes::CreationOutcomesConfig;
mod admission_log;
mod anonymous_public;
mod auth_mode;
mod debug;
mod global_admission;
mod http_connections;
mod provider_tool_relay;
mod public_owner_sandbox_limit;
pub(crate) use access_migration::AgentAccessMigration;
#[cfg(test)]
pub(crate) use anonymous_public::AnonymousPublicConfig;
pub(crate) use provider_tool_relay::ProviderToolRelay;
mod agent_manager;
mod agent_sandbox;
pub(crate) use agent_manager::AgentManagerReplicas;
mod execution;
mod gateway;
mod home;
mod oauth_resource;
mod profiles;
mod public_streams;
mod rate_default_lease;
mod rate_limits;
pub use rate_limits::RateLimitConfig;
mod relay_capacity;
mod runtime;
mod runtime_capacity;
mod runtime_migration;
mod shell_settings;
pub(crate) use relay_capacity::RelayCapacityConfig;
pub(crate) use shell_settings::BackendFreeConfig;

mod feature_switches;
mod features;
mod quiet_output;
pub(crate) mod task_audit;
mod task_experience;
pub(crate) use cma_am_auth::service_config::ServiceAuthorizationConfig;
pub(crate) use features::home_backup::HomeBackupConfig;
pub(crate) use features::{FeatureFlags, ManagedMachineUpdates};
pub(crate) use gateway::GatewayConfig;
mod bot_father;
mod browser_session;
mod documents;
pub(crate) mod report_views;
mod sandbox_billing;
mod sandbox_usage_export;
pub(crate) use sandbox_usage_export::UsageCaptureConfig;
pub(crate) mod sandbox_metrics;
mod usage_billing;
mod usage_transport;
pub(crate) use bot_father::BotFatherConfig;
pub(crate) use documents::DocumentStorageConfig;
pub(crate) use profiles::DefaultProfileConfig;
pub(crate) use runtime::{canary as runtime_canary, RuntimeConfig};
pub(crate) use sandbox_billing::SandboxBillingConfig;
pub(crate) use sandbox_metrics::MetricsConfig;
pub(crate) use usage_billing::UsageBillingConfig;
pub(crate) use usage_transport::UsageExportConfig;

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_PUBLIC_URL: &str = "http://localhost:8080";
const DEFAULT_CORS_ORIGIN: &str = "http://localhost:5173";
const DEFAULT_DATABASE: &str = "chrono_bot";
pub(crate) const MONGO_DIAGNOSTICS_ENABLED: &str = "CHRONO_MONGO_DIAGNOSTICS_ENABLED";
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
    pub(crate) creation_outcomes: Option<CreationOutcomesConfig>,
    pub(crate) creation_writer_epoch: Option<String>,
    pub(crate) account_deletion_enabled: bool,
    /// Published terms and privacy notice; `None` records no consent.
    pub(crate) legal_terms: Option<layer2_core::legal_consent::LegalTerms>,
    pub(crate) retain_legal_consents: bool,
    pub(crate) account_erasure_transaction_drain_seconds: u64,
    pub(crate) global_sandbox_limit: u16,
    pub(crate) public_owner_sandbox_limit: Option<std::num::NonZeroU16>,
    pub(crate) agent_log_fleet_percent: u8,
    pub(crate) bounded_admission_agents: Vec<layer2_core::AgentId>,
    pub bind: SocketAddr,
    pub(crate) http_connection_max_age: Option<std::time::Duration>,
    pub metrics_bind: SocketAddr,
    pub(crate) task_audit: Option<task_audit::Selection>,
    pub public_url: String,
    /// Explicit prior authorities for credentials already installed in live Agents.
    pub agent_access_retained_origins: Vec<String>,
    pub(crate) agent_access_transport_migration: Option<AgentAccessMigration>,
    pub(crate) provider_tool_relay: Option<ProviderToolRelay>,
    /// Base URL for the independently deployed Image Manager service.  CMA
    /// only uses it with the request's delegated user credential.
    pub image_manager_base_url: String,
    pub nyxid_public_url: Option<String>,
    pub nyxid_service_id: Option<String>,
    pub nyxid_oauth_api_resource: Option<String>,
    pub nyxid_proxy_issuer: Option<String>,
    pub nyxid_proxy_jwks_url: Option<String>,
    pub nyxid_proxy_audience: Option<String>,
    pub cors_origins: Vec<HeaderValue>,
    pub rate_limits: RateLimitConfig,
    pub(crate) control_admission: ControlAdmissionConfig,
    pub(crate) public_stream_limit: usize,
    pub(crate) relay_capacity: RelayCapacityConfig,
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
    pub(crate) startup_schema_mode: StartupSchemaMode,
    pub lifecycle_failure_metadata_enabled: bool,
    pub activity_async_enabled: bool,
    pub activity_operational_read_sampling_enabled: bool,
    pub default_profiles: DefaultProfileConfig,
    pub agent_manager_sandbox_capacity: layer3_adapter::SandboxCapacity,
    pub desktop_verified_runtime_image: Option<String>,
    pub(crate) public_repository_verified_runtime_images: BTreeSet<String>,
    pub(crate) public_repository_snapshot_enabled: bool,
    pub auth_mode: AuthMode,
    pub browser_session_join_enabled: bool,
    pub features: FeatureFlags,
    pub(crate) home_backup: HomeBackupConfig,
    pub(crate) service_authorization: Option<ServiceAuthorizationConfig>,
    pub(crate) agent_manager_replicas: Option<AgentManagerReplicas>,
    pub task_experience:
        std::sync::Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    pub gateway: Option<GatewayConfig>,
    pub usage_billing: Option<UsageBillingConfig>,
    pub(crate) usage_capture: Option<UsageCaptureConfig>,
    pub(crate) usage_export: Option<UsageExportConfig>,
    pub(crate) usage_metrics: MetricsConfig,
    pub(crate) usage_measured: bool,
    pub sandbox_billing: Option<SandboxBillingConfig>,
    pub bot_father: Option<BotFatherConfig>,
    pub document_storage: Option<DocumentStorageConfig>,
}

pub(crate) enum StartupConfig {
    Execution(Box<Config>),
    BackendFree(Box<BackendFreeConfig>),
}

impl StartupConfig {
    pub(crate) fn from_env() -> Result<Self, String> {
        Self::from_lookup(strict_env)
    }

    pub(crate) fn from_lookup(
        lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let mut lookup = feature_switches::with_bundle(lookup)?;
        match runtime::RuntimeMode::from_lookup(&mut lookup)? {
            runtime::RuntimeMode::Execution(provider) => Config::with_provider(lookup, provider)
                .map(|config| Self::Execution(Box::new(config))),
            runtime::RuntimeMode::BackendFree => BackendFreeConfig::from_lookup(lookup)
                .map(|config| Self::BackendFree(Box::new(config))),
        }
    }
}

fn parse_public_repository_images(raw: Option<String>) -> Result<BTreeSet<String>, String> {
    let mut images = BTreeSet::new();
    for image in raw
        .as_deref()
        .filter(|value| !value.is_empty())
        .into_iter()
        .flat_map(|value| value.split(','))
    {
        if !crate::agent_lifecycle::valid_immutable_image(image) || !images.insert(image.to_owned())
        {
            return Err("CHRONO_PUBLIC_REPOSITORY_VERIFIED_RUNTIME_IMAGES requires unique immutable image references".into());
        }
    }
    Ok(images)
}

impl Config {
    /// Read and validate configuration exactly once at process startup.
    #[cfg(test)]
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(strict_env)
    }

    #[cfg(test)]
    fn from_lookup(
        lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        match StartupConfig::from_lookup(lookup)? {
            StartupConfig::Execution(config) => Ok(*config),
            StartupConfig::BackendFree(_) => {
                Err("an execution backend is required for runtime configuration".into())
            }
        }
    }

    fn with_provider(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
        provider: runtime::Provider,
    ) -> Result<Self, String> {
        let shell_settings::ServerSettings {
            bind,
            metrics_bind,
            public_url,
            cors_origins,
        } = shell_settings::server(&mut lookup)?;
        let agent_access_retained_origins = access_origins::retained(&mut lookup)?;
        let agent_access_transport_migration = access_migration::parse(&mut lookup)?;
        let provider_tool_relay = provider_tool_relay::parse(&mut lookup, &public_url)?;
        if agent_access_transport_migration
            .as_ref()
            .is_some_and(|migration| {
                migration.origin.trim_end_matches('/') != public_url.trim_end_matches('/')
            })
        {
            return Err("CHRONO_AGENT_ACCESS_TRANSPORT_MIGRATION origin must match CHRONO_SERVER_PUBLIC_URL".into());
        }
        let image_manager_base_url = parse_service_base_url(
            "CHRONO_IMAGE_MANAGER_BASE_URL",
            &value_or(
                &mut lookup,
                "CHRONO_IMAGE_MANAGER_BASE_URL",
                &format!("{}/api/image-manager", public_url.trim_end_matches('/')),
            )?,
        )?;
        let features = FeatureFlags::from_lookup(&mut lookup)?;
        if features.home_backup_expiry_enabled {
            return Err("CHRONO_HOME_BACKUP_EXPIRY_ENABLED requires a qualified deletion provider; retention preparation cannot be enabled".into());
        }
        let home_backup = HomeBackupConfig::from_lookup(&mut lookup, features.home_backup_enabled)?;
        let service_authorization =
            ServiceAuthorizationConfig::from_lookup(&mut lookup, features.service_authorization)?;
        let account_deletion_enabled = account_deletion::enabled(&mut lookup)?;
        let legal_terms = legal_terms::from_lookup(&mut lookup)?;
        let creation_outcomes = CreationOutcomesConfig::from_lookup(&mut lookup)?;
        let creation_writer_epoch = CreationOutcomesConfig::writer_epoch(&mut lookup)?;
        let retain_legal_consents = account_deletion::retain_legal_consents(&mut lookup)?;
        let account_erasure_transaction_drain_seconds =
            account_deletion::transaction_drain(&mut lookup)?;
        let global_sandbox_limit = global_admission::from_lookup(&mut lookup)?;
        let public_owner_sandbox_limit = public_owner_sandbox_limit::from_lookup(&mut lookup)?;
        let bounded_admission_agents = admission_log::from_lookup(&mut lookup)?;
        let agent_log_fleet_percent =
            admission_log::fleet_percent(&mut lookup, &bounded_admission_agents)?;
        let rate_limits = RateLimitConfig::from_lookup(&mut lookup)?;
        let trusted_proxies = TrustedProxyConfig::from_lookup(&mut lookup)?;
        let shell_settings::DatabaseSettings {
            mongodb_uri,
            mongodb_database,
            mongodb_authority,
            deployment_id,
        } = shell_settings::database(&mut lookup)?;
        let runtime = RuntimeConfig::from_lookup(&mut lookup, provider)?;
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
        let activity_retention = shell_settings::activity_retention(&mut lookup)?;
        let default_profiles = DefaultProfileConfig::from_lookup(&mut lookup)?;
        let agent_manager_sandbox_capacity = profiles::agent_manager_capacity(&mut lookup)?;

        let shell_settings::AuthenticationSettings {
            auth_mode,
            nyxid_public_url,
        } = shell_settings::authentication(&mut lookup, &public_url)?;
        let document_storage = DocumentStorageConfig::from_lookup(&mut lookup)?;
        if agent_access_transport_migration.is_some() && document_storage.is_none() {
            return Err(
                "CHRONO_AGENT_ACCESS_TRANSPORT_MIGRATION requires managed document context".into(),
            );
        }
        let agent_manager_replicas = AgentManagerReplicas::from_lookup(&mut lookup)?;
        if features.agent_shared_observation && agent_manager_replicas.is_none() {
            return Err("CHRONO_AGENT_SHARED_OBSERVATION_ENABLED requires CHRONO_AGENT_MANAGER_REPLICAS_ENABLED".into());
        }
        Ok(Self {
            global_sandbox_limit,
            public_owner_sandbox_limit,
            account_deletion_enabled,
            legal_terms,
            creation_outcomes,
            creation_writer_epoch,
            retain_legal_consents,
            account_erasure_transaction_drain_seconds,
            bind,
            http_connection_max_age: http_connections::max_age(&mut lookup)?,
            metrics_bind,
            task_audit: task_audit::Selection::from_lookup(&mut lookup)?,
            public_stream_limit: public_streams::from_lookup(&mut lookup)?,
            relay_capacity: RelayCapacityConfig::from_lookup(&mut lookup)?,
            public_url,
            agent_access_retained_origins,
            agent_access_transport_migration,
            provider_tool_relay,
            image_manager_base_url,
            nyxid_public_url,
            nyxid_service_id: lookup("CHRONO_NYXID_SERVICE_ID")?.filter(|id| !id.trim().is_empty()),
            nyxid_oauth_api_resource: oauth_resource::parse(lookup(
                "CHRONO_NYXID_OAUTH_API_RESOURCE",
            )?)?,
            nyxid_proxy_issuer: lookup("CHRONO_NYXID_PROXY_ISSUER")?,
            nyxid_proxy_jwks_url: lookup("CHRONO_NYXID_PROXY_JWKS_URL")?,
            nyxid_proxy_audience: lookup("CHRONO_NYXID_PROXY_AUDIENCE")?,
            cors_origins,
            rate_limits,
            control_admission: ControlAdmissionConfig::from_lookup(&mut lookup)?,
            trusted_proxies,
            mongodb_uri,
            mongodb_database,
            mongodb_authority,
            deployment_id,
            runtime,
            execution,
            baseline_services,
            auth_encryption_key,
            activity_retention,
            startup_schema_mode: StartupSchemaMode::from_lookup(&mut lookup)?,
            lifecycle_failure_metadata_enabled: lifecycle_failure::enabled(&mut lookup)?,
            activity_async_enabled: activity::enabled(&mut lookup)?,
            activity_operational_read_sampling_enabled: activity::sampling_enabled(&mut lookup)?,
            default_profiles,
            agent_manager_sandbox_capacity,
            desktop_verified_runtime_image: lookup("CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE")?
                .filter(|value| !value.is_empty()),
            public_repository_snapshot_enabled: features::flag(
                &mut lookup,
                "CHRONO_PUBLIC_REPOSITORY_SNAPSHOT_ENABLED",
            )?,
            public_repository_verified_runtime_images: parse_public_repository_images(lookup(
                "CHRONO_PUBLIC_REPOSITORY_VERIFIED_RUNTIME_IMAGES",
            )?)?,
            auth_mode,
            browser_session_join_enabled: browser_session::from_lookup(&mut lookup)?,
            features,
            home_backup,
            service_authorization,
            agent_manager_replicas,
            task_experience: task_experience::from_lookup(&mut lookup)?,
            bounded_admission_agents,
            agent_log_fleet_percent,
            gateway: GatewayConfig::from_lookup(&mut lookup)?,
            usage_billing: UsageBillingConfig::from_lookup(&mut lookup)?,
            usage_capture: UsageCaptureConfig::from_lookup(&mut lookup)?,
            usage_export: UsageExportConfig::from_lookup(&mut lookup)?,
            usage_metrics: MetricsConfig::from_lookup(&mut lookup)?,
            usage_measured: sandbox_usage_export::measured_enabled(&mut lookup)?,
            sandbox_billing: SandboxBillingConfig::from_lookup(&mut lookup)?,
            bot_father: BotFatherConfig::from_lookup(&mut lookup)?,
            document_storage,
        })
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
#[cfg(test)]
pub(crate) use tests::{desktop_canary_config, retained_runtime_config};

#[cfg(test)]
mod graduation_tests;
