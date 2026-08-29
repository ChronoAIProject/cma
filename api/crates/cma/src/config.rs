//! Configuration, read from the environment at startup.
//!
//! Every key here also appears in the deployment's ConfigMap or Secret. A key
//! read here but absent there is a divergence that will only show up in
//! production, so the two must be changed together.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use axum::http::HeaderValue;
use ipnet::IpNet;
use layer2_core::{
    RequiredCapability, SponsorshipGate, SponsorshipGateFence, TriggerRolloutPolicy,
};

use crate::auth::AuthMode;
use crate::store::AuthEncryptionKey;

const MONGODB_CUTOVER_STAGE: &str = "CHRONO_MONGODB_CUTOVER_STAGE";
const ADMISSION_RATE_REQUESTS_PER_MINUTE: u32 = 6_000;
const HEALTH_RATE_REQUESTS_PER_MINUTE: u32 = 120;
const MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE: u32 = 30;
const DEFAULT_RATE_REQUESTS_PER_MINUTE: u32 = 600;
const CREATE_RATE_REQUESTS_PER_MINUTE: u32 = 10;
const INPUT_RATE_REQUESTS_PER_MINUTE: u32 = 120;
const DEFAULT_TRUSTED_PROXY_HOPS: usize = 1;
const MAX_TRUSTED_PROXY_HOPS: usize = 16;
const PINNED_CLOUDFLARE_PROXY_CIDRS: &str = concat!(
    "173.245.48.0/20,103.21.244.0/22,103.22.200.0/22,103.31.4.0/22,",
    "141.101.64.0/18,108.162.192.0/18,190.93.240.0/20,188.114.96.0/20,",
    "197.234.240.0/22,198.41.128.0/17,162.158.0.0/15,104.16.0.0/13,",
    "104.24.0.0/14,172.64.0.0/13,131.0.72.0/22,2400:cb00::/32,",
    "2606:4700::/32,2803:f800::/32,2405:b500::/32,2405:8100::/32,",
    "2a06:98c0::/29,2c0f:f248::/32",
);
const DEFAULT_TRIGGER_CHANGED_FILE_LIMIT: usize = 1_000;
const DEFAULT_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS: usize = 300;
const MAX_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS: usize = 3_600;
// These are compile-time stops, not activation defaults. They keep one future
// public policy within reviewed service-scale concurrency/rate bounds and force
// a code review before one policy can authorize more than $100 or 10M tokens.
const MIN_SPONSORSHIP_POLICY_EXPIRY_SECONDS: u64 = 60;
const MAX_SPONSORSHIP_POLICY_EXPIRY_SECONDS: u64 = 86_400;
const MAX_SPONSORSHIP_ACTIVE_POLICIES_PER_SPONSOR: u32 = 32;
const MIN_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS: u64 = 60;
const MAX_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS: u64 = 3_600;
const MAX_SPONSORSHIP_LAUNCHES_PER_SPONSOR_WINDOW: u32 = 100;
const MAX_SPONSORSHIP_CONCURRENT_LAUNCHES: u32 = 32;
const MAX_SPONSORSHIP_POLICY_REQUESTS: u64 = 10_000;
const MAX_SPONSORSHIP_PER_LAUNCH_REQUESTS: u64 = 1_000;
const MAX_SPONSORSHIP_REQUESTS_PER_MINUTE: u32 = 600;
const MAX_SPONSORSHIP_POLICY_TOKENS: u64 = 10_000_000;
const MAX_SPONSORSHIP_PER_LAUNCH_TOKENS: u64 = 1_000_000;
const MAX_SPONSORSHIP_POLICY_BUDGET_MICROS: u64 = 100_000_000;
const MAX_SPONSORSHIP_PER_LAUNCH_BUDGET_MICROS: u64 = 10_000_000;
const MAX_SPONSORSHIP_PROVIDER_POLICY_KEYS: usize = 32;
const MAX_SPONSORSHIP_WORKER_INTERVAL_SECONDS: u64 = 300;
const MAX_SPONSORSHIP_WORKER_MAX_PAGES: usize = 100;
const MAX_SPONSORSHIP_WORKER_CONCURRENCY: usize = 32;
const MAX_FILE_SECRET_BYTES: usize = 65_536;
const SPONSORSHIP_NOT_COMPOSED_ERROR: &str = "CHRONO_SPONSORSHIP_ENABLED=true is not available in this build because sponsorship persistence and provider, billing, and pricing adapters are not composed; keep CHRONO_SPONSORSHIP_ENABLED=false and CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED=true";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerCleanupMode {
    DryRun,
    Execute,
}

impl TriggerCleanupMode {
    pub fn code(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Execute => "execute",
        }
    }
}

/// Deployment-owned Trigger rollout, scheduling, and retention controls.
#[derive(Debug, Clone)]
pub struct TriggerRuntimeConfig {
    pub rollout: TriggerRolloutPolicy,
    /// NyxID public-proxy service exposing only credential-free repository metadata.
    pub public_github_service_slug: Option<String>,
    pub runtime_capabilities: Vec<RequiredCapability>,
    pub cleanup_mode: TriggerCleanupMode,
    pub changed_file_limit: u32,
    pub fork_readiness_timeout: std::time::Duration,
    pub interval: std::time::Duration,
    pub page_size: u32,
    pub max_pages: usize,
    pub concurrency: usize,
}

impl TriggerRuntimeConfig {
    fn from_env() -> Result<Self, String> {
        let environment_enabled = strict_bool(
            "CHRONO_TRIGGER_ENABLED",
            &var_or("CHRONO_TRIGGER_ENABLED", "false"),
        )?;
        let orchestration_paused = strict_bool(
            "CHRONO_TRIGGER_ORCHESTRATION_PAUSED",
            &var_or("CHRONO_TRIGGER_ORCHESTRATION_PAUSED", "true"),
        )?;
        let capability_kill_switch = strict_bool(
            "CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH",
            &var_or("CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH", "true"),
        )?;
        let publisher_subject_ids = list_or("CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS", "");
        let repository_ids = parse_u64_list(
            "CHRONO_TRIGGER_REPOSITORY_IDS",
            &var_or("CHRONO_TRIGGER_REPOSITORY_IDS", ""),
        )?;
        let user_cohort_percent = parse_bounded_usize(
            "CHRONO_TRIGGER_USER_COHORT_PERCENT",
            &var_or("CHRONO_TRIGGER_USER_COHORT_PERCENT", "0"),
            0,
            100,
        )? as u8;
        let rollout = TriggerRolloutPolicy::new(
            environment_enabled,
            orchestration_paused,
            capability_kill_switch,
            publisher_subject_ids,
            repository_ids,
            user_cohort_percent,
        )
        .map_err(|error| error.to_string())?;
        let public_github_service_slug = parse_optional_service_slug(
            "CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG",
            &var_or("CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG", ""),
        )?;
        let runtime_capabilities = parse_trigger_runtime_capabilities(
            "CHRONO_TRIGGER_RUNTIME_CAPABILITIES",
            &var_or("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", ""),
        )?;
        let cleanup_mode = parse_trigger_cleanup_mode(
            "CHRONO_TRIGGER_CLEANUP_MODE",
            &var_or("CHRONO_TRIGGER_CLEANUP_MODE", "dry_run"),
        )?;
        let changed_file_limit = parse_bounded_usize(
            "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
            &var_or(
                "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
                &DEFAULT_TRIGGER_CHANGED_FILE_LIMIT.to_string(),
            ),
            1,
            layer2_core::MAX_DIFF_SUMMARY_FILES,
        )? as u32;
        let fork_readiness_timeout_seconds = parse_bounded_usize(
            "CHRONO_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS",
            &var_or(
                "CHRONO_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS",
                &DEFAULT_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS.to_string(),
            ),
            1,
            MAX_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS,
        )?;
        /*
         * Keep the scheduler's operation budget bounded independently of the
         * number of stored Launches. Values outside these ceilings fail
         * startup rather than turning a typo into an unbounded provider pass.
         */
        let interval_seconds = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS",
            &var_or("CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS", "5"),
            1,
            300,
        )?;
        let page_size = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_PAGE_SIZE",
            &var_or("CHRONO_TRIGGER_WORKER_PAGE_SIZE", "50"),
            1,
            layer2_core::MAX_TRIGGER_PAGE_SIZE as usize,
        )? as u32;
        let max_pages = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_MAX_PAGES",
            &var_or("CHRONO_TRIGGER_WORKER_MAX_PAGES", "20"),
            1,
            100,
        )?;
        let concurrency = parse_bounded_usize(
            "CHRONO_TRIGGER_WORKER_CONCURRENCY",
            &var_or("CHRONO_TRIGGER_WORKER_CONCURRENCY", "4"),
            1,
            32,
        )?;
        Ok(Self {
            rollout,
            public_github_service_slug,
            runtime_capabilities,
            cleanup_mode,
            changed_file_limit,
            fork_readiness_timeout: std::time::Duration::from_secs(
                fork_readiness_timeout_seconds as u64,
            ),
            interval: std::time::Duration::from_secs(interval_seconds as u64),
            page_size,
            max_pages,
            concurrency,
        })
    }

    #[cfg(test)]
    fn sample() -> Self {
        Self {
            rollout: TriggerRolloutPolicy::allow_all(),
            public_github_service_slug: Some("api-github".into()),
            runtime_capabilities: all_trigger_runtime_capabilities(),
            cleanup_mode: TriggerCleanupMode::DryRun,
            changed_file_limit: DEFAULT_TRIGGER_CHANGED_FILE_LIMIT as u32,
            fork_readiness_timeout: std::time::Duration::from_secs(
                DEFAULT_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS as u64,
            ),
            interval: std::time::Duration::from_secs(5),
            page_size: 50,
            max_pages: 20,
            concurrency: 4,
        }
    }
}

fn parse_optional_service_slug(key: &str, raw: &str) -> Result<Option<String>, String> {
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > 63
        || raw.starts_with('-')
        || raw.ends_with('-')
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "{key} must be one lowercase service slug containing only letters, digits, and hyphens"
        ));
    }
    Ok(Some(raw.to_string()))
}

fn parse_trigger_cleanup_mode(key: &str, raw: &str) -> Result<TriggerCleanupMode, String> {
    match raw {
        "dry_run" => Ok(TriggerCleanupMode::DryRun),
        "execute" => Ok(TriggerCleanupMode::Execute),
        other => Err(format!(
            "{key} must be \"dry_run\" or \"execute\", got {other:?}"
        )),
    }
}

#[cfg(test)]
fn all_trigger_runtime_capabilities() -> Vec<RequiredCapability> {
    vec![
        RequiredCapability::Conversation,
        RequiredCapability::DurableConversation,
        RequiredCapability::WorkspaceCheckpoint,
        RequiredCapability::RestrictedServiceGateway,
        RequiredCapability::CredentialRefresh,
        RequiredCapability::CredentialRevocation,
    ]
}

fn parse_trigger_runtime_capabilities(
    key: &str,
    raw: &str,
) -> Result<Vec<RequiredCapability>, String> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut parsed = Vec::new();
    for name in raw.split(',') {
        let capability = match name {
            "conversation" => RequiredCapability::Conversation,
            "durable_conversation" => RequiredCapability::DurableConversation,
            "workspace_checkpoint" => RequiredCapability::WorkspaceCheckpoint,
            "restricted_service_gateway" => RequiredCapability::RestrictedServiceGateway,
            "credential_refresh" => RequiredCapability::CredentialRefresh,
            "credential_revocation" => RequiredCapability::CredentialRevocation,
            _ => {
                return Err(format!(
                    "{key} contains unsupported capability {name:?}; use exact comma-separated canonical names"
                ));
            }
        };
        if parsed.contains(&capability) {
            return Err(format!("{key} contains duplicate capability {name:?}"));
        }
        parsed.push(capability);
    }
    Ok(parsed)
}

/// Deployment-owned ceilings that every sponsored policy must fit within.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipLimitsConfig {
    pub default_policy_expiry: std::time::Duration,
    pub maximum_policy_expiry: std::time::Duration,
    pub maximum_active_policies_per_sponsor: u32,
    pub launch_rate_window: std::time::Duration,
    pub maximum_launches_per_sponsor_per_window: u32,
    pub maximum_per_launch_requests: u64,
    pub maximum_requests_per_minute: u32,
    pub maximum_per_launch_input_tokens: u64,
    pub maximum_per_launch_output_tokens: u64,
    pub maximum_policy_input_tokens: u64,
    pub maximum_policy_output_tokens: u64,
    pub maximum_per_launch_budget_micros: u64,
}

/// Reviewed identities and HTTPS origins for the three sponsorship ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipAdaptersConfig {
    pub provider_adapter_id: String,
    pub provider_base_url: String,
    pub billing_adapter_id: String,
    pub billing_base_url: String,
    pub pricing_adapter_id: String,
    pub pricing_base_url: String,
}

/// Bounded work one sponsorship reconciliation tick may claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipWorkerConfig {
    pub interval: std::time::Duration,
    pub page_size: u32,
    pub max_pages: usize,
    pub concurrency: usize,
}

/// Secret classes used only by sponsorship persistence and privacy-safe audit.
#[allow(dead_code)] // Store and audit adapters are intentionally outside issue #671's config slice.
#[derive(Clone)]
pub struct SponsorshipSecretsConfig {
    pub encryption_keyring: zeroize::Zeroizing<String>,
    pub audit_hmac_key: zeroize::Zeroizing<String>,
}

impl std::fmt::Debug for SponsorshipSecretsConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorshipSecretsConfig")
            .field("encryption_keyring", &"<redacted>")
            .field("audit_hmac_key", &"<redacted>")
            .finish()
    }
}

/// Complete disabled sponsorship staging configuration.
///
/// The ordinary production default is `None`. A `Some` value means an operator
/// supplied and validated the entire disabled, kill-switched tuple. This build
/// refuses enabled configuration until its stores and adapters are composed.
#[allow(dead_code)] // This issue declares the dormant tuple without composing adapters.
#[derive(Debug, Clone)]
pub struct SponsorshipRuntimeConfig {
    pub gate: SponsorshipGate,
    pub fence: SponsorshipGateFence,
    pub trusted_provider_policy_key: String,
    pub limits: SponsorshipLimitsConfig,
    pub adapters: SponsorshipAdaptersConfig,
    pub worker: SponsorshipWorkerConfig,
    pub secrets: SponsorshipSecretsConfig,
}

impl SponsorshipRuntimeConfig {
    fn from_env() -> Result<Option<Self>, String> {
        Self::from_lookup(strict_env)
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let enabled = strict_bool(
            "CHRONO_SPONSORSHIP_ENABLED",
            &lookup_or(&mut lookup, "CHRONO_SPONSORSHIP_ENABLED", "false")?,
        )?;
        if enabled {
            return Err(SPONSORSHIP_NOT_COMPOSED_ERROR.into());
        }
        let kill_switch_engaged = strict_bool(
            "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED",
            &lookup_or(
                &mut lookup,
                "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED",
                "true",
            )?,
        )?;

        let epoch = optional_from(&mut lookup, "CHRONO_SPONSORSHIP_EPOCH")?;
        let configuration_digest =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST")?;
        let expiry_evidence_id =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_EXPIRY_EVIDENCE_ID")?;
        let revocation_evidence_id =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_REVOCATION_EVIDENCE_ID")?;
        let trusted_provider_policy_key = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY",
        )?;
        let allowed_provider_policy_keys = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS",
        )?;
        let maximum_policy_budget_micros = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS",
        )?;
        let maximum_policy_requests =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS")?;
        let maximum_concurrent_launches = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES",
        )?;
        let default_policy_expiry_seconds = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS",
        )?;
        let maximum_policy_expiry_seconds = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS",
        )?;
        let maximum_active_policies_per_sponsor = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR",
        )?;
        let launch_rate_window_seconds =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS")?;
        let maximum_launches_per_sponsor_per_window = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW",
        )?;
        let maximum_per_launch_requests = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS",
        )?;
        let maximum_requests_per_minute = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE",
        )?;
        let maximum_per_launch_input_tokens = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
        )?;
        let maximum_per_launch_output_tokens = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
        )?;
        let maximum_policy_input_tokens = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS",
        )?;
        let maximum_policy_output_tokens = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS",
        )?;
        let maximum_per_launch_budget_micros = optional_from(
            &mut lookup,
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
        )?;
        let provider_adapter_id =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID")?;
        let provider_base_url = optional_from(&mut lookup, "CHRONO_SPONSORSHIP_PROVIDER_BASE_URL")?;
        let billing_adapter_id =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID")?;
        let billing_base_url = optional_from(&mut lookup, "CHRONO_SPONSORSHIP_BILLING_BASE_URL")?;
        let pricing_adapter_id =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID")?;
        let pricing_base_url = optional_from(&mut lookup, "CHRONO_SPONSORSHIP_PRICING_BASE_URL")?;
        let worker_interval_seconds =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS")?;
        let worker_page_size = optional_from(&mut lookup, "CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE")?;
        let worker_max_pages = optional_from(&mut lookup, "CHRONO_SPONSORSHIP_WORKER_MAX_PAGES")?;
        let worker_concurrency =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_WORKER_CONCURRENCY")?;
        let encryption_keyring_file =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE")?;
        let audit_hmac_key_file =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE")?;
        let encryption_keyring_inline =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING")?
                .map(zeroize::Zeroizing::new);
        let audit_hmac_key_inline =
            optional_from(&mut lookup, "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY")?
                .map(zeroize::Zeroizing::new);

        let supplied = [
            &epoch,
            &configuration_digest,
            &expiry_evidence_id,
            &revocation_evidence_id,
            &trusted_provider_policy_key,
            &allowed_provider_policy_keys,
            &maximum_policy_budget_micros,
            &maximum_policy_requests,
            &maximum_concurrent_launches,
            &default_policy_expiry_seconds,
            &maximum_policy_expiry_seconds,
            &maximum_active_policies_per_sponsor,
            &launch_rate_window_seconds,
            &maximum_launches_per_sponsor_per_window,
            &maximum_per_launch_requests,
            &maximum_requests_per_minute,
            &maximum_per_launch_input_tokens,
            &maximum_per_launch_output_tokens,
            &maximum_policy_input_tokens,
            &maximum_policy_output_tokens,
            &maximum_per_launch_budget_micros,
            &provider_adapter_id,
            &provider_base_url,
            &billing_adapter_id,
            &billing_base_url,
            &pricing_adapter_id,
            &pricing_base_url,
            &worker_interval_seconds,
            &worker_page_size,
            &worker_max_pages,
            &worker_concurrency,
            &encryption_keyring_file,
            &audit_hmac_key_file,
        ]
        .into_iter()
        .any(Option::is_some);

        if encryption_keyring_inline.is_some() {
            return Err(
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING is not accepted; mount the dedicated sponsorship keyring and set CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE"
                    .into(),
            );
        }
        if audit_hmac_key_inline.is_some() {
            return Err(
                "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY is not accepted; mount the dedicated sponsorship audit key and set CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE"
                    .into(),
            );
        }
        if kill_switch_engaged && !supplied {
            return Ok(None);
        }
        if !kill_switch_engaged {
            return Err(
                "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED cannot be false while CHRONO_SPONSORSHIP_ENABLED=false"
                    .into(),
            );
        }

        let epoch = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_EPOCH",
            &required_sponsorship_value(epoch, "CHRONO_SPONSORSHIP_EPOCH")?,
            1,
            i64::MAX as u64,
        )?;
        let configuration_digest = required_sponsorship_value(
            configuration_digest,
            "CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST",
        )?;
        let expiry_evidence_id = required_sponsorship_value(
            expiry_evidence_id,
            "CHRONO_SPONSORSHIP_EXPIRY_EVIDENCE_ID",
        )?;
        let revocation_evidence_id = required_sponsorship_value(
            revocation_evidence_id,
            "CHRONO_SPONSORSHIP_REVOCATION_EVIDENCE_ID",
        )?;
        let trusted_provider_policy_key = parse_sponsorship_policy_token(
            "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY",
            &required_sponsorship_value(
                trusted_provider_policy_key,
                "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY",
            )?,
        )?;
        let allowed_provider_policy_keys = parse_sponsorship_policy_tokens(
            "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS",
            &required_sponsorship_value(
                allowed_provider_policy_keys,
                "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS",
            )?,
        )?;
        if !allowed_provider_policy_keys.contains(&trusted_provider_policy_key) {
            return Err(
                "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY must be present in CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS"
                    .into(),
            );
        }
        let maximum_policy_budget_micros = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS",
            &required_sponsorship_value(
                maximum_policy_budget_micros,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS",
            )?,
            1,
            MAX_SPONSORSHIP_POLICY_BUDGET_MICROS,
        )?;
        let maximum_policy_requests = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS",
            &required_sponsorship_value(
                maximum_policy_requests,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS",
            )?,
            1,
            MAX_SPONSORSHIP_POLICY_REQUESTS,
        )?;
        let maximum_concurrent_launches = parse_bounded_u32(
            "CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES",
            &required_sponsorship_value(
                maximum_concurrent_launches,
                "CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES",
            )?,
            1,
            MAX_SPONSORSHIP_CONCURRENT_LAUNCHES,
        )?;

        let gate = SponsorshipGate {
            enabled,
            expiry_evidence_id: Some(expiry_evidence_id),
            revocation_evidence_id: Some(revocation_evidence_id),
            allowed_provider_policy_keys,
            maximum_policy_budget_micros,
            maximum_policy_requests,
            maximum_concurrent_launches,
            kill_switch_engaged,
        };
        let validation_gate = SponsorshipGate {
            enabled: true,
            kill_switch_engaged: false,
            ..gate.clone()
        };
        validation_gate.validate().map_err(|error| {
            format!("the configured CHRONO_SPONSORSHIP deployment gate is invalid: {error}")
        })?;
        let expected_digest = gate.configuration_digest().map_err(|error| {
            format!("the CHRONO_SPONSORSHIP deployment gate cannot be digested: {error}")
        })?;
        if configuration_digest != expected_digest {
            return Err(
                "CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST does not match the configured sponsorship gate"
                    .into(),
            );
        }
        let fence = SponsorshipGateFence {
            configuration_digest,
            epoch,
            kill_switch_engaged,
        };
        fence.validate().map_err(|error| {
            format!("the configured CHRONO_SPONSORSHIP fence is invalid: {error}")
        })?;
        let default_policy_expiry_seconds = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS",
            &required_sponsorship_value(
                default_policy_expiry_seconds,
                "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS",
            )?,
            MIN_SPONSORSHIP_POLICY_EXPIRY_SECONDS,
            MAX_SPONSORSHIP_POLICY_EXPIRY_SECONDS,
        )?;
        let maximum_policy_expiry_seconds = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS",
            &required_sponsorship_value(
                maximum_policy_expiry_seconds,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS",
            )?,
            MIN_SPONSORSHIP_POLICY_EXPIRY_SECONDS,
            MAX_SPONSORSHIP_POLICY_EXPIRY_SECONDS,
        )?;
        if default_policy_expiry_seconds > maximum_policy_expiry_seconds {
            return Err(
                "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS must not exceed CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS"
                    .into(),
            );
        }
        let maximum_active_policies_per_sponsor = parse_bounded_u32(
            "CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR",
            &required_sponsorship_value(
                maximum_active_policies_per_sponsor,
                "CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR",
            )?,
            1,
            MAX_SPONSORSHIP_ACTIVE_POLICIES_PER_SPONSOR,
        )?;
        let launch_rate_window_seconds = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS",
            &required_sponsorship_value(
                launch_rate_window_seconds,
                "CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS",
            )?,
            MIN_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS,
            MAX_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS,
        )?;
        let maximum_launches_per_sponsor_per_window = parse_bounded_u32(
            "CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW",
            &required_sponsorship_value(
                maximum_launches_per_sponsor_per_window,
                "CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW",
            )?,
            1,
            MAX_SPONSORSHIP_LAUNCHES_PER_SPONSOR_WINDOW,
        )?;
        let maximum_per_launch_requests = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS",
            &required_sponsorship_value(
                maximum_per_launch_requests,
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS",
            )?,
            1,
            MAX_SPONSORSHIP_PER_LAUNCH_REQUESTS,
        )?;
        let maximum_requests_per_minute = parse_bounded_u32(
            "CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE",
            &required_sponsorship_value(
                maximum_requests_per_minute,
                "CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE",
            )?,
            1,
            MAX_SPONSORSHIP_REQUESTS_PER_MINUTE,
        )?;
        let maximum_per_launch_input_tokens = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
            &required_sponsorship_value(
                maximum_per_launch_input_tokens,
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
            )?,
            1,
            MAX_SPONSORSHIP_PER_LAUNCH_TOKENS,
        )?;
        let maximum_per_launch_output_tokens = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
            &required_sponsorship_value(
                maximum_per_launch_output_tokens,
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
            )?,
            1,
            MAX_SPONSORSHIP_PER_LAUNCH_TOKENS,
        )?;
        let maximum_policy_input_tokens = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS",
            &required_sponsorship_value(
                maximum_policy_input_tokens,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS",
            )?,
            1,
            MAX_SPONSORSHIP_POLICY_TOKENS,
        )?;
        let maximum_policy_output_tokens = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS",
            &required_sponsorship_value(
                maximum_policy_output_tokens,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS",
            )?,
            1,
            MAX_SPONSORSHIP_POLICY_TOKENS,
        )?;
        let maximum_per_launch_budget_micros = parse_bounded_u64(
            "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
            &required_sponsorship_value(
                maximum_per_launch_budget_micros,
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
            )?,
            1,
            MAX_SPONSORSHIP_PER_LAUNCH_BUDGET_MICROS,
        )?;
        for (per_launch_key, per_launch, policy_key, policy) in [
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS",
                maximum_per_launch_requests,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS",
                maximum_policy_requests,
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
                maximum_per_launch_input_tokens,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS",
                maximum_policy_input_tokens,
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
                maximum_per_launch_output_tokens,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS",
                maximum_policy_output_tokens,
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
                maximum_per_launch_budget_micros,
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS",
                maximum_policy_budget_micros,
            ),
        ] {
            if per_launch > policy {
                return Err(format!("{per_launch_key} must not exceed {policy_key}"));
            }
        }
        let limits = SponsorshipLimitsConfig {
            default_policy_expiry: std::time::Duration::from_secs(default_policy_expiry_seconds),
            maximum_policy_expiry: std::time::Duration::from_secs(maximum_policy_expiry_seconds),
            maximum_active_policies_per_sponsor,
            launch_rate_window: std::time::Duration::from_secs(launch_rate_window_seconds),
            maximum_launches_per_sponsor_per_window,
            maximum_per_launch_requests,
            maximum_requests_per_minute,
            maximum_per_launch_input_tokens,
            maximum_per_launch_output_tokens,
            maximum_policy_input_tokens,
            maximum_policy_output_tokens,
            maximum_per_launch_budget_micros,
        };

        let adapters = SponsorshipAdaptersConfig {
            provider_adapter_id: parse_sponsorship_policy_token(
                "CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID",
                &required_sponsorship_value(
                    provider_adapter_id,
                    "CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID",
                )?,
            )?,
            provider_base_url: parse_canonical_https_origin(
                "CHRONO_SPONSORSHIP_PROVIDER_BASE_URL",
                &required_sponsorship_value(
                    provider_base_url,
                    "CHRONO_SPONSORSHIP_PROVIDER_BASE_URL",
                )?,
            )?,
            billing_adapter_id: parse_sponsorship_policy_token(
                "CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID",
                &required_sponsorship_value(
                    billing_adapter_id,
                    "CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID",
                )?,
            )?,
            billing_base_url: parse_canonical_https_origin(
                "CHRONO_SPONSORSHIP_BILLING_BASE_URL",
                &required_sponsorship_value(
                    billing_base_url,
                    "CHRONO_SPONSORSHIP_BILLING_BASE_URL",
                )?,
            )?,
            pricing_adapter_id: parse_sponsorship_policy_token(
                "CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID",
                &required_sponsorship_value(
                    pricing_adapter_id,
                    "CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID",
                )?,
            )?,
            pricing_base_url: parse_canonical_https_origin(
                "CHRONO_SPONSORSHIP_PRICING_BASE_URL",
                &required_sponsorship_value(
                    pricing_base_url,
                    "CHRONO_SPONSORSHIP_PRICING_BASE_URL",
                )?,
            )?,
        };

        let worker = SponsorshipWorkerConfig {
            interval: std::time::Duration::from_secs(parse_bounded_u64(
                "CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS",
                &required_sponsorship_value(
                    worker_interval_seconds,
                    "CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS",
                )?,
                1,
                MAX_SPONSORSHIP_WORKER_INTERVAL_SECONDS,
            )?),
            page_size: parse_bounded_u32(
                "CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE",
                &required_sponsorship_value(
                    worker_page_size,
                    "CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE",
                )?,
                1,
                layer2_core::MAX_SPONSORED_AUDIT_PAGE as u32,
            )?,
            max_pages: parse_bounded_usize(
                "CHRONO_SPONSORSHIP_WORKER_MAX_PAGES",
                &required_sponsorship_value(
                    worker_max_pages,
                    "CHRONO_SPONSORSHIP_WORKER_MAX_PAGES",
                )?,
                1,
                MAX_SPONSORSHIP_WORKER_MAX_PAGES,
            )?,
            concurrency: parse_bounded_usize(
                "CHRONO_SPONSORSHIP_WORKER_CONCURRENCY",
                &required_sponsorship_value(
                    worker_concurrency,
                    "CHRONO_SPONSORSHIP_WORKER_CONCURRENCY",
                )?,
                1,
                MAX_SPONSORSHIP_WORKER_CONCURRENCY,
            )?,
        };

        let encryption_keyring_file = required_sponsorship_value(
            encryption_keyring_file,
            "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE",
        )?;
        let audit_hmac_key_file = required_sponsorship_value(
            audit_hmac_key_file,
            "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE",
        )?;
        ensure_distinct_secret_files(
            "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE",
            &encryption_keyring_file,
            "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE",
            &audit_hmac_key_file,
        )?;
        let encryption_keyring = zeroize::Zeroizing::new(
            optional_file_secret_from_values(
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE",
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING",
                Some(encryption_keyring_file),
                None,
            )?
            .ok_or_else(|| {
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE did not resolve a mounted secret"
                    .to_string()
            })?,
        );
        let audit_hmac_key = zeroize::Zeroizing::new(
            optional_file_secret_from_values(
                "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE",
                "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY",
                Some(audit_hmac_key_file),
                None,
            )?
            .ok_or_else(|| {
                "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE did not resolve a mounted secret"
                    .to_string()
            })?,
        );
        for (key, value) in [
            (
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE",
                &encryption_keyring,
            ),
            ("CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE", &audit_hmac_key),
        ] {
            if value.len() > MAX_FILE_SECRET_BYTES {
                return Err(format!(
                    "{key} exceeds the {MAX_FILE_SECRET_BYTES}-byte secret-file limit"
                ));
            }
        }
        if encryption_keyring.as_str() == audit_hmac_key.as_str() {
            return Err(
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE and CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE must contain independent secret material"
                    .into(),
            );
        }
        let secrets = SponsorshipSecretsConfig {
            encryption_keyring,
            audit_hmac_key,
        };

        Ok(Some(Self {
            gate,
            fence,
            trusted_provider_policy_key,
            limits,
            adapters,
            worker,
            secrets,
        }))
    }
}

fn optional_from(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
) -> Result<Option<String>, String> {
    Ok(lookup(key)?.filter(|value| !value.trim().is_empty()))
}

fn required_sponsorship_value(value: Option<String>, key: &str) -> Result<String, String> {
    value.ok_or_else(|| {
        format!("{key} is required when any CHRONO_SPONSORSHIP configuration is staged or enabled")
    })
}

fn parse_sponsorship_policy_token(key: &str, raw: &str) -> Result<String, String> {
    if raw.is_empty()
        || raw.len() > 128
        || !raw.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(format!("{key} must be a 1-128 byte canonical policy token"));
    }
    Ok(raw.to_owned())
}

fn parse_sponsorship_policy_tokens(key: &str, raw: &str) -> Result<BTreeSet<String>, String> {
    let mut values = BTreeSet::new();
    for value in raw.split(',') {
        let value = parse_sponsorship_policy_token(key, value)?;
        if !values.insert(value.clone()) {
            return Err(format!("{key} contains duplicate policy key {value:?}"));
        }
        if values.len() > MAX_SPONSORSHIP_PROVIDER_POLICY_KEYS {
            return Err(format!(
                "{key} must contain at most {MAX_SPONSORSHIP_PROVIDER_POLICY_KEYS} policy keys"
            ));
        }
    }
    if values.is_empty() {
        return Err(format!("{key} must contain at least one policy key"));
    }
    Ok(values)
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
        return Err(format!(
            "{key} must be one canonical absolute HTTPS origin without credentials, path, query, or fragment"
        ));
    }
    Ok(canonical)
}

fn ensure_distinct_secret_files(
    left_key: &str,
    left_path: &str,
    right_key: &str,
    right_path: &str,
) -> Result<(), String> {
    let left = std::fs::canonicalize(left_path)
        .map_err(|error| format!("cannot resolve {left_key} at {left_path}: {error}"))?;
    let right = std::fs::canonicalize(right_path)
        .map_err(|error| format!("cannot resolve {right_key} at {right_path}: {error}"))?;
    if left == right {
        return Err(format!(
            "{left_key} and {right_key} must name separate mounted files"
        ));
    }
    Ok(())
}

/// A deliberately incomplete control-plane startup used at a recovery boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MongoCutoverStage {
    /// Migrate only the legacy session namespaces, then remain quiescent.
    Legacy,
}

/// The only configuration a staged MongoDB cutover is allowed to observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MongoCutoverStageConfig {
    /// The stage selected by the operator.
    pub stage: MongoCutoverStage,
    /// Address for the maintenance-only health listener.
    pub bind: String,
    /// Connection string for the one store the stage opens.
    pub mongodb_uri: String,
    /// Database whose legacy namespaces are migrated.
    pub mongodb_database: String,
}

impl MongoCutoverStageConfig {
    /// Read a staged startup before the normal control-plane configuration.
    pub fn from_env() -> Result<Option<Self>, String> {
        Self::from_lookup(strict_env)
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let stage = lookup(MONGODB_CUTOVER_STAGE)?.unwrap_or_default();
        let stage = match stage.as_str() {
            "" => return Ok(None),
            "legacy" => MongoCutoverStage::Legacy,
            _ => {
                return Err(format!(
                    "{MONGODB_CUTOVER_STAGE} must be unset or exactly \"legacy\""
                ))
            }
        };

        let legacy = lookup("CHRONO_MONGODB_LEGACY_CUTOVER")?.unwrap_or_default();
        if legacy != "drained" {
            return Err(
                "CHRONO_MONGODB_CUTOVER_STAGE=legacy requires CHRONO_MONGODB_LEGACY_CUTOVER=drained after every legacy writer and its in-flight work have stopped"
                    .into(),
            );
        }
        let coordination = lookup("CHRONO_MONGODB_COORDINATION_CUTOVER")?.unwrap_or_default();
        if !coordination.is_empty() {
            return Err(
                "CHRONO_MONGODB_CUTOVER_STAGE=legacy requires CHRONO_MONGODB_COORDINATION_CUTOVER to be unset so coordination activation cannot begin"
                    .into(),
            );
        }

        let bind = lookup_or(&mut lookup, "CHRONO_SERVER_BIND", "0.0.0.0:8080")?;
        let mongodb_uri = require_from(&mut lookup, "CHRONO_MONGODB_URI")?;
        let mongodb_database = lookup_or(&mut lookup, "CHRONO_MONGODB_DATABASE", "chrono_bot")?;
        Ok(Some(Self {
            stage,
            bind,
            mongodb_uri,
            mongodb_database,
        }))
    }
}

/// Per-key request budgets enforced by this process.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    /// Cheap per-network admission decisions made before authentication.
    pub admission_per_minute: NonZeroU32,
    /// Health requests, isolated from caller traffic so probes remain viable.
    pub health_per_minute: NonZeroU32,
    /// Requests per minute for routes without a stricter operation budget.
    pub default_per_minute: NonZeroU32,
    /// Session creations per minute.
    pub create_per_minute: NonZeroU32,
    /// Session input deliveries per minute.
    pub input_per_minute: NonZeroU32,
}

impl RateLimitConfig {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            admission_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_ADMISSION_PER_MINUTE",
                ADMISSION_RATE_REQUESTS_PER_MINUTE,
            )?,
            health_per_minute: u32_at_least(
                "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
                HEALTH_RATE_REQUESTS_PER_MINUTE,
                MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE,
            )?,
            default_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_DEFAULT_PER_MINUTE",
                DEFAULT_RATE_REQUESTS_PER_MINUTE,
            )?,
            create_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_CREATE_PER_MINUTE",
                CREATE_RATE_REQUESTS_PER_MINUTE,
            )?,
            input_per_minute: positive_u32(
                "CHRONO_SERVER_RATE_INPUT_PER_MINUTE",
                INPUT_RATE_REQUESTS_PER_MINUTE,
            )?,
        })
    }

    #[cfg(test)]
    fn sample() -> Self {
        Self {
            admission_per_minute: NonZeroU32::new(ADMISSION_RATE_REQUESTS_PER_MINUTE).unwrap(),
            health_per_minute: NonZeroU32::new(HEALTH_RATE_REQUESTS_PER_MINUTE).unwrap(),
            default_per_minute: NonZeroU32::new(DEFAULT_RATE_REQUESTS_PER_MINUTE).unwrap(),
            create_per_minute: NonZeroU32::new(CREATE_RATE_REQUESTS_PER_MINUTE).unwrap(),
            input_per_minute: NonZeroU32::new(INPUT_RATE_REQUESTS_PER_MINUTE).unwrap(),
        }
    }
}

/// Which TCP peers may supply the load balancer's client-address chain.
#[derive(Debug, Clone)]
pub struct TrustedProxyConfig {
    /// GFE networks from which a forwarding header is accepted.
    pub cidrs: Vec<IpNet>,
    /// GFE-owned tokens to skip from the right before selecting its observed
    /// upstream address.
    pub forwarded_hops: usize,
    /// Cloudflare edge networks allowed to authenticate the next-left visitor
    /// token, subject to exact `CF-Connecting-IP` agreement.
    pub cloudflare_cidrs: Vec<IpNet>,
}

impl TrustedProxyConfig {
    fn from_env() -> Result<Self, String> {
        let cidrs = parse_ip_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            &var_or("CHRONO_SERVER_TRUSTED_PROXY_CIDRS", ""),
        )?;
        let forwarded_hops = parse_bounded_usize(
            "CHRONO_SERVER_TRUSTED_PROXY_HOPS",
            &var_or(
                "CHRONO_SERVER_TRUSTED_PROXY_HOPS",
                &DEFAULT_TRUSTED_PROXY_HOPS.to_string(),
            ),
            1,
            MAX_TRUSTED_PROXY_HOPS,
        )?;
        let cloudflare_cidrs = parse_ip_networks(
            "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS",
            &var_or("CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS", ""),
        )?;
        validate_cloudflare_proxy_networks(&cidrs, &cloudflare_cidrs)?;
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

/// Resolved configuration for the control plane.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind.
    pub bind: String,
    /// This service's public origin, used to build redirect URIs.
    pub public_url: String,
    /// Origins permitted to call this API from a browser.
    pub cors_origins: Vec<HeaderValue>,
    /// Per-key request budgets enforced by this process.
    pub rate_limits: RateLimitConfig,
    /// Peers whose load-balancer forwarding chain may identify a client.
    pub trusted_proxies: TrustedProxyConfig,
    /// Base URL of the sandbox provider's lifecycle API.
    pub sandbox_base_url: String,
    /// API key for the sandbox provider.
    pub sandbox_api_key: String,
    /// Dedicated provider lane for Trigger-owned Codex runtimes.
    ///
    /// `None` keeps Trigger admission fail-closed while leaving ordinary
    /// sessions available. The separate endpoint is what lets OpenSandbox
    /// apply a compatible secure runtime without changing the runtime class of
    /// every other session on this deployment.
    pub trigger_sandbox_provider: Option<SandboxProviderConfig>,
    /// Image each sandbox runs.
    pub sandbox_image: String,
    /// CPU a sandbox reserves when its session names no size, in Kubernetes quantity form.
    ///
    /// Configuration rather than a constant because it decides how many
    /// sessions a deployment can run at once: the namespace has a quota, and
    /// this figure divides into it. Sizing that from a rebuild means a session
    /// nobody can start until an image ships.
    pub sandbox_cpu: String,
    /// Memory a sandbox reserves when its session names no size, in Kubernetes quantity form.
    ///
    /// The tighter of the two in practice. The agent is what runs here, and
    /// an allowance it cannot work within is not a smaller session but a
    /// session that dies partway through somebody's work.
    pub sandbox_memory: String,
    /// What this deployment calls itself on the runtimes it creates.
    ///
    /// A sandbox provider is shared, so one key sees runtimes belonging to
    /// other control planes. This is what tells them apart.
    pub deployment_id: String,
    /// Whether runtimes carrying no deployment name are this one's to reclaim.
    ///
    /// True for the deployment that owns the provider, so runtimes created
    /// before names were recorded are still reclaimed once orphaned. Any
    /// additional control plane sharing the provider must set this false, or it
    /// will tear down sessions belonging to the first.
    pub adopts_untagged_runtimes: bool,
    /// Software every session carries, whoever started it.
    ///
    /// Part of what a session *is* on this platform rather than something a
    /// caller opted into, which is why an environment profile cannot remove an
    /// entry listed here.
    pub baseline_packages: Vec<String>,
    /// How the agent behaves in a session that names no profile.
    ///
    /// The deployment's answer to "start me a session" when nobody has said
    /// anything more. Configuration rather than a constant because which model
    /// is worth spending, and how freely an agent may act before asking, are
    /// decisions about this installation and not about the software.
    pub default_agent: layer2_core::AgentConfiguration,
    /// Services every session's credential may reach, whoever started it.
    ///
    /// A floor rather than a default. One credential serves the agent's model
    /// calls and the platform CLI alike, so a profile that excluded the model
    /// endpoint would leave the session unable to think rather than merely
    /// narrower.
    pub baseline_services: Vec<String>,
    /// Skills every session loads, whoever started it.
    ///
    /// Identified the way the registry identifies a skill: either its
    /// generated identifier or its unique name, both of which resolve.
    pub baseline_skills: Vec<String>,
    pub upstream_base_url: String,
    /// Model the agent selects.
    pub upstream_model: String,
    /// Request shape the upstream speaks.
    pub upstream_wire_api: String,
    /// Where a sandbox forwards the second kernel's model calls.
    ///
    /// Falls back to the shared upstream when a deployment names none, which is
    /// the ordinary case for a gateway that serves both request shapes at one
    /// address.
    pub claude_upstream_base_url: String,
    /// Model the second kernel selects when a session names none.
    pub claude_upstream_model: String,
    /// Where a sandbox forwards the third kernel's model calls.
    ///
    /// Falls back to the shared upstream when a deployment names none.
    pub dsh_upstream_base_url: String,
    /// Model the third kernel selects when a session names none.
    pub dsh_upstream_model: String,
    /// Root of the identity platform a session's CLI acts against.
    pub platform_base_url: String,
    /// Connection string for the durable session store.
    pub mongodb_uri: String,
    /// Database holding session records and their history.
    pub mongodb_database: String,
    /// Explicit acknowledgement for the one-time legacy namespace cutover.
    ///
    /// This does not discover or stop old writers. It is accepted only when an
    /// operator has already stopped every pre-cutover control plane and waited
    /// for its MongoDB operations to drain.
    pub mongodb_legacy_cutover: bool,
    /// Explicit acknowledgement that every pre-coordination process and its
    /// in-flight provider operations have permanently drained.
    pub mongodb_coordination_cutover: bool,
    /// Dedicated key for recoverable authentication secrets stored in MongoDB.
    pub auth_encryption_key: AuthEncryptionKey,
    /// How long a recorded action is kept.
    ///
    /// Configuration rather than a constant because how far back a deployment
    /// must be able to account for itself is a decision about that deployment.
    /// It reaches the database as an index, so lowering it removes what is
    /// already stored rather than only affecting what is stored next.
    pub activity_retention: std::time::Duration,
    /// Bucket holding stored session context.
    ///
    /// Named explicitly rather than defaulted, and the only bucket this
    /// service ever addresses. It creates no bucket and holds no capability to
    /// create one; an absent name switches context storage off.
    pub context_bucket: String,
    /// Bucket holding Trigger repository baselines and workspace checkpoints.
    ///
    /// Separate configuration from conversation context even when a deployment
    /// deliberately selects the same owned bucket. Repository artifacts have
    /// their own retention and deletion policy, and must never inherit the
    /// transcript lifecycle merely because both use platform object storage.
    pub trigger_workspace_bucket: String,
    /// GitHub Trigger rollout and bounded worker controls.
    pub trigger: TriggerRuntimeConfig,
    /// Complete sponsorship controls, absent in the dormant deployment.
    #[allow(dead_code)]
    // Sponsorship has no runtime composition until its evidence gates land.
    pub sponsorship: Option<SponsorshipRuntimeConfig>,
    /// Current Ed25519 signer for short-lived runtime policies, when enabled.
    pub github_capability_signing: Option<GithubCapabilitySigningConfig>,
    /// This service's own machine identity on the identity platform, used to
    /// store session context.
    ///
    /// Its own identity rather than a credential borrowed from elsewhere. The
    /// sandbox provider and the identity platform are separate trust domains,
    /// and a key issued for one must never be spendable at the other.
    ///
    /// The identifier is not secret; the secret is, and the two are delivered
    /// separately for that reason.
    pub storage_client_id: String,
    /// Secret half of the machine identity above.
    pub storage_client_secret: String,
    /// How callers are authenticated.
    pub auth_mode: AuthMode,
}

/// One sandbox-provider authority held by the composition root.
#[derive(Clone)]
pub struct SandboxProviderConfig {
    pub base_url: String,
    pub api_key: String,
    /// Namespace containing this provider lane's BatchSandbox resources.
    ///
    /// This is both discovery scope and deletion authority. It is never
    /// inferred from the ordinary session lane for Trigger runtimes.
    pub namespace: String,
}

impl std::fmt::Debug for SandboxProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SandboxProviderConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("namespace", &self.namespace)
            .finish()
    }
}

/// Secret signing material and public-key rotation inputs.
#[derive(Clone)]
pub struct GithubCapabilitySigningConfig {
    pub key_id: String,
    pub private_key_pkcs8_base64: String,
    pub previous_verification_keys_json: String,
}

impl std::fmt::Debug for GithubCapabilitySigningConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GithubCapabilitySigningConfig")
            .field("key_id", &self.key_id)
            .field("private_key_pkcs8_base64", &"<redacted>")
            .field(
                "previous_verification_keys_json",
                &self.previous_verification_keys_json,
            )
            .finish()
    }
}

/// Services every session's credential may reach when a deployment names none.
///
/// Named the way a person reads them; the identifiers the provider wants are
/// resolved when a credential is minted.
///
/// `ornn` and `ornn-api` are two services, not one name spelled twice.
/// `ornn-api` answers the skill registry this service itself searches to show
/// somebody what they may pick; `ornn` is the skill runtime the agent calls
/// once it has picked, and it is the one the baseline skills tell it to reach
/// for. A baseline naming only the registry mints a credential the provider
/// refuses on every call the agent actually makes.
///
/// A constant rather than a literal at the use site so the default is a thing
/// a test can hold, instead of a string a test would have to repeat — which
/// would pin the test to itself and nothing else.
const BASELINE_SERVICES: &str = "chrono-llm,ornn-api,ornn";

const DEFAULT_SANDBOX_IMAGE: &str = "cma-runtime:dev";

/// Deployment identity used only when a new installation omits the key.
///
/// Existing installations set this explicitly because it is persisted into
/// credential names and sandbox metadata; a rebrand must not change that
/// state identity underneath them.
const DEFAULT_DEPLOYMENT_ID: &str = "cma";

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> Result<Self, String> {
        let bind = var_or("CHRONO_SERVER_BIND", "0.0.0.0:8080");
        let public_url = var_or("CHRONO_SERVER_PUBLIC_URL", "http://localhost:8080");
        let rate_limits = RateLimitConfig::from_env()?;
        let trusted_proxies = TrustedProxyConfig::from_env()?;
        let sandbox_base_url = var_or(
            "CHRONO_SANDBOX_BASE_URL",
            "http://opensandbox-server.opensandbox-system.svc.cluster.local",
        );
        let sandbox_image = var_or("CHRONO_SANDBOX_IMAGE", DEFAULT_SANDBOX_IMAGE);
        // What a sandbox costs the namespace quota when its session names no
        // size. Read as opaque strings and handed to the provider unchanged:
        // these are Kubernetes quantities, and parsing them here would only
        // add a way to be wrong about a syntax this service does not own.
        let sandbox_cpu = var_or("CHRONO_SANDBOX_CPU", "1");
        let sandbox_memory = var_or("CHRONO_SANDBOX_MEMORY", "1Gi");
        // How the agent is told to continue a conversation rather than start
        // one. Here rather than baked into the runtime image so a change in
        // the agent's own flags is a configuration edit, not a rebuild.
        let deployment_id = var_or("CHRONO_DEPLOYMENT_ID", DEFAULT_DEPLOYMENT_ID);
        let adopts_untagged_runtimes =
            var_or("CHRONO_ADOPT_UNTAGGED_RUNTIMES", "true").trim() != "false";
        // Defaulted rather than required. These are what a session is expected
        // to have on this platform, and an environment that forgot to name them
        // must not silently produce sessions without them. A deployment that
        // genuinely mandates nothing sets the keys to a single space.
        let baseline_packages = list_or("CHRONO_BASELINE_PACKAGES", "nyxid");
        let baseline_skills = list_or("CHRONO_BASELINE_SKILLS", "nyxid,ornn-agent-manual-cli");
        let baseline_services = list_or("CHRONO_BASELINE_SERVICES", BASELINE_SERVICES);
        // What a session is when nobody chose anything. Each key is read on
        // its own and a value the software does not recognise is refused
        // rather than silently replaced: a deployment that meant "ask first"
        // and typed it wrong must be told, not quietly made autonomous.
        let default_agent = layer2_core::AgentConfiguration {
            kernel: layer2_core::KernelId::new(var_or("CHRONO_DEFAULT_AGENT_KERNEL", "codex")),
            // Conversation, not terminal. The conversation plane is the
            // surface this product is for: it is where approvals are answered,
            // where tool calls read as structure, and where a failure can be
            // explained rather than printed. A terminal is the escape hatch.
            //
            // Only the deployment default moves. `SessionMode`'s serde default
            // stays Terminal deliberately — it is what every record written
            // before the field existed means, and reading those back as
            // conversations would silently change what they are.
            mode: parse_enum("CHRONO_DEFAULT_AGENT_MODE", "conversation")?,
            model: Some(var_or("CHRONO_DEFAULT_AGENT_MODEL", "gpt-5.6-sol"))
                .filter(|m| !m.trim().is_empty()),
            reasoning_effort: parse_enum("CHRONO_DEFAULT_AGENT_EFFORT", "high")?,
            permission: parse_enum("CHRONO_DEFAULT_AGENT_PERMISSION", "guarded")?,
            credential: parse_enum("CHRONO_DEFAULT_AGENT_CREDENTIAL", "delegated")?,
            // No key for this, deliberately. `None` already means
            // CHRONO_SANDBOX_CPU / CHRONO_SANDBOX_MEMORY, so a
            // CHRONO_DEFAULT_AGENT_SIZE would be a second way to say the same
            // thing that could disagree with the first.
            size: None,
            record_tool_bodies: false,
            // The baseline skills are what every session carries anyway, so a
            // profile-less session carries exactly them and nothing else.
            skills: list_or("CHRONO_BASELINE_SKILLS", "nyxid,ornn-agent-manual-cli")
                .into_iter()
                .map(layer2_core::SkillId::new)
                .collect(),
        };
        let upstream_base_url = var_or("CHRONO_UPSTREAM_BASE_URL", "");
        let upstream_model = var_or("CHRONO_UPSTREAM_MODEL", "gpt-5.6-sol");
        let upstream_wire_api = var_or("CHRONO_UPSTREAM_WIRE_API", "responses");
        // The second kernel's upstream. Its own keys because kernels do not
        // share a model and need not share an endpoint: one speaks a
        // completions-shaped API and the other a messages-shaped one, and a
        // deployment whose gateway serves both at one address simply leaves the
        // base URL unset and inherits the first.
        let claude_upstream_base_url = match var_or("CHRONO_CLAUDE_UPSTREAM_BASE_URL", "") {
            empty if empty.is_empty() => upstream_base_url.clone(),
            named => named,
        };
        let claude_upstream_model = var_or("CHRONO_CLAUDE_UPSTREAM_MODEL", "claude-opus-5");
        // The third kernel's, on the same terms and for the same reason.
        let dsh_upstream_base_url = match var_or("CHRONO_DSH_UPSTREAM_BASE_URL", "") {
            empty if empty.is_empty() => upstream_base_url.clone(),
            named => named,
        };
        let dsh_upstream_model = var_or("CHRONO_DSH_UPSTREAM_MODEL", "deepseek-v4-flash");
        // Defaults to the same provider this service authenticates against, so
        // a session acts on the platform as the person who signed in.
        let platform_base_url = var_or("CHRONO_PLATFORM_BASE_URL", "");
        // Required: sessions are durable, and a service that starts without a
        // store would accept work it silently cannot keep.
        let mongodb_uri = require("CHRONO_MONGODB_URI")?;
        let mongodb_database = var_or("CHRONO_MONGODB_DATABASE", "chrono_bot");
        let mongodb_legacy_cutover = parse_cutover_drain(
            "CHRONO_MONGODB_LEGACY_CUTOVER",
            &var_or("CHRONO_MONGODB_LEGACY_CUTOVER", ""),
        )?;
        let mongodb_coordination_cutover = parse_cutover_drain(
            "CHRONO_MONGODB_COORDINATION_CUTOVER",
            &var_or("CHRONO_MONGODB_COORDINATION_CUTOVER", ""),
        )?;
        let auth_encryption_key =
            AuthEncryptionKey::from_base64(&require("CHRONO_AUTH_ENCRYPTION_KEY")?)?;
        // Ninety days by default: long enough to investigate something nobody
        // noticed at the time, short enough that a collection holding one
        // document per request does not grow without limit. A deployment with
        // a longer obligation raises it deliberately.
        let activity_retention = days("CHRONO_ACTIVITY_RETENTION_DAYS", 90)?;
        // No default. The bucket is created and owned by whoever runs the
        // platform, and every object this service writes lands in it, so
        // guessing a name would mean writing somewhere nobody chose.
        let context_bucket = var_or("CHRONO_CONTEXT_BUCKET", "");
        let trigger_workspace_bucket = var_or("CHRONO_TRIGGER_WORKSPACE_BUCKET", "");
        let trigger = TriggerRuntimeConfig::from_env()?;
        let sponsorship = SponsorshipRuntimeConfig::from_env()?;
        let capability_key_id = var_or("CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID", "");
        let capability_private_key = var_or("CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8", "");
        let previous_verification_keys_json =
            var_or("CHRONO_GITHUB_CAPABILITY_PREVIOUS_VERIFY_KEYS", "{}");
        let github_capability_signing = match (
            capability_key_id.is_empty(),
            capability_private_key.is_empty(),
        ) {
            (true, true) => None,
            (false, false) => Some(GithubCapabilitySigningConfig {
                key_id: capability_key_id,
                private_key_pkcs8_base64: capability_private_key,
                previous_verification_keys_json,
            }),
            _ => {
                return Err("CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID and \
                     CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8 must be set together"
                    .into())
            }
        };
        let storage_client_id = var_or("CHRONO_STORAGE_CLIENT_ID", "");
        let storage_client_secret = var_or("CHRONO_STORAGE_CLIENT_SECRET", "");
        // The key is read from a file rather than an environment variable so a
        // mounted secret keeps it out of the pod spec and process environment.
        // Configuration is resolved once at startup; adopting a rotated file
        // therefore requires a controlled process restart.
        let sandbox_api_key = match std::env::var("CHRONO_SANDBOX_API_KEY_FILE")
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            Some(path) => std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read CHRONO_SANDBOX_API_KEY_FILE at {path}: {e}"))?
                .trim()
                .to_string(),
            None => require("CHRONO_SANDBOX_API_KEY")?,
        };
        let trigger_sandbox_base_url = strict_env("CHRONO_TRIGGER_SANDBOX_BASE_URL")?
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().trim_end_matches('/').to_string());
        let trigger_sandbox_namespace = strict_env("CHRONO_TRIGGER_SANDBOX_NAMESPACE")?
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string());
        let trigger_sandbox_api_key = optional_file_secret(
            "CHRONO_TRIGGER_SANDBOX_API_KEY_FILE",
            "CHRONO_TRIGGER_SANDBOX_API_KEY",
        )?;
        let trigger_sandbox_provider = resolve_trigger_sandbox_provider(
            trigger.rollout.environment_enabled,
            &sandbox_base_url,
            &sandbox_api_key,
            trigger_sandbox_base_url,
            trigger_sandbox_namespace,
            trigger_sandbox_api_key,
        )?;

        let cors_origins = var_or("CHRONO_SERVER_CORS_ORIGIN", "http://localhost:5173")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                HeaderValue::from_str(s).map_err(|e| {
                    format!("CHRONO_SERVER_CORS_ORIGIN entry {s:?} is not usable: {e}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Trusted mode has to be asked for by name. Defaulting to it would mean
        // a missing provider configuration silently disables authentication.
        let auth_mode = match var_or("CHRONO_AUTH_MODE", "provider").as_str() {
            "trusted" => AuthMode::Trusted {
                subject: var_or("CHRONO_TRUSTED_SUBJECT", "local-developer"),
            },
            "provider" => {
                let base_url = require("CHRONO_NYXID_BASE_URL")?;
                let client_id = require("CHRONO_NYXID_CLIENT_ID")?;
                let client_secret = require("CHRONO_NYXID_CLIENT_SECRET")?;
                AuthMode::Provider {
                    base_url,
                    client_id,
                    client_secret,
                    public_url: public_url.clone(),
                }
            }
            other => {
                return Err(format!(
                    "CHRONO_AUTH_MODE must be \"provider\" or \"trusted\", got {other:?}"
                ))
            }
        };

        Ok(Self {
            bind,
            public_url,
            cors_origins,
            rate_limits,
            trusted_proxies,
            sandbox_base_url,
            sandbox_api_key,
            trigger_sandbox_provider,
            sandbox_image,
            sandbox_cpu,
            sandbox_memory,
            default_agent,
            deployment_id,
            adopts_untagged_runtimes,
            baseline_packages,
            baseline_skills,
            baseline_services,
            upstream_base_url,
            upstream_model,
            upstream_wire_api,
            claude_upstream_base_url,
            claude_upstream_model,
            dsh_upstream_base_url,
            dsh_upstream_model,
            platform_base_url,
            mongodb_uri,
            mongodb_database,
            mongodb_legacy_cutover,
            mongodb_coordination_cutover,
            auth_encryption_key,
            activity_retention,
            context_bucket,
            trigger_workspace_bucket,
            trigger,
            sponsorship,
            github_capability_signing,
            storage_client_id,
            storage_client_secret,
            auth_mode,
        })
    }

    /// Name of the configured auth mode, for logging.
    pub fn auth_mode_name(&self) -> &'static str {
        match self.auth_mode {
            AuthMode::Provider { .. } => "provider",
            AuthMode::Trusted { .. } => "trusted",
        }
    }
}

fn var_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn strict_env(key: &str) -> Result<Option<String>, String> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "{key} is not valid UTF-8 and cannot be interpreted"
        )),
    }
}

fn lookup_or(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    Ok(lookup(key)?
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned()))
}

fn require_from(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
) -> Result<String, String> {
    lookup(key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required and was not set"))
}

fn require(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{key} is required and was not set"))
}

fn optional_file_secret(
    file_key: &str,
    forbidden_inline_key: &str,
) -> Result<Option<String>, String> {
    optional_file_secret_from_values(
        file_key,
        forbidden_inline_key,
        strict_env(file_key)?,
        strict_env(forbidden_inline_key)?,
    )
}

fn optional_file_secret_from_values(
    file_key: &str,
    forbidden_inline_key: &str,
    path: Option<String>,
    inline: Option<String>,
) -> Result<Option<String>, String> {
    let inline = inline.map(zeroize::Zeroizing::new);
    if inline
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(format!(
            "{forbidden_inline_key} is not accepted; mount the dedicated provider key and set {file_key}"
        ));
    }
    if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
        let value = zeroize::Zeroizing::new(
            std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {file_key} at {path}: {error}"))?,
        );
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{file_key} at {path} contains an empty secret"));
        }
        return Ok(Some(value.to_string()));
    }
    Ok(None)
}

fn resolve_trigger_sandbox_provider(
    trigger_enabled: bool,
    ordinary_base_url: &str,
    ordinary_api_key: &str,
    base_url: Option<String>,
    namespace: Option<String>,
    api_key: Option<String>,
) -> Result<Option<SandboxProviderConfig>, String> {
    match (base_url, namespace, api_key) {
        (Some(base_url), Some(namespace), Some(api_key)) => {
            let parsed = reqwest::Url::parse(&base_url).map_err(|_| {
                "CHRONO_TRIGGER_SANDBOX_BASE_URL must be an absolute HTTP(S) provider origin"
                    .to_string()
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.path() != "/"
            {
                return Err(
                    "CHRONO_TRIGGER_SANDBOX_BASE_URL must be an absolute HTTP(S) provider origin"
                        .into(),
                );
            }
            if same_http_origin(&parsed, ordinary_base_url) {
                return Err(
                    "CHRONO_TRIGGER_SANDBOX_BASE_URL must not reuse the ordinary sandbox provider origin"
                        .into(),
                );
            }
            if api_key == ordinary_api_key {
                return Err(
                    "the Trigger sandbox provider key must not reuse the ordinary sandbox provider key"
                        .into(),
                );
            }
            if !is_dns1123_label(&namespace) {
                return Err(
                    "CHRONO_TRIGGER_SANDBOX_NAMESPACE must be one lowercase DNS-1123 label"
                        .into(),
                );
            }
            Ok(Some(SandboxProviderConfig {
                base_url,
                api_key,
                namespace,
            }))
        }
        (None, None, None) if !trigger_enabled => Ok(None),
        (None, None, None) => Err(
            "CHRONO_TRIGGER_SANDBOX_BASE_URL, CHRONO_TRIGGER_SANDBOX_NAMESPACE, and CHRONO_TRIGGER_SANDBOX_API_KEY_FILE are required when CHRONO_TRIGGER_ENABLED=true"
                .into(),
        ),
        _ => Err(
            "CHRONO_TRIGGER_SANDBOX_BASE_URL, CHRONO_TRIGGER_SANDBOX_NAMESPACE, and CHRONO_TRIGGER_SANDBOX_API_KEY_FILE must be set together"
                .into(),
        ),
    }
}

fn same_http_origin(left: &reqwest::Url, right: &str) -> bool {
    let Ok(right) = reqwest::Url::parse(right) else {
        return false;
    };
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_dns1123_label(value: &str) -> bool {
    let bytes = value.as_bytes();
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= 63
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes.iter().all(|byte| edge(*byte) || *byte == b'-')
}

fn strict_bool(key: &str, raw: &str) -> Result<bool, String> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!(
            "{key} must be exactly \"true\" or \"false\", got {raw:?}"
        )),
    }
}

fn parse_u64_list(key: &str, raw: &str) -> Result<Vec<u64>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|part| {
            let part = part.trim();
            let value = part.parse::<u64>().map_err(|error| {
                format!("{key} contains invalid numeric identity {part:?}: {error}")
            })?;
            if value == 0 {
                return Err(format!("{key} contains invalid numeric identity 0"));
            }
            Ok(value)
        })
        .collect()
}

fn positive_u32(key: &str, default: u32) -> Result<NonZeroU32, String> {
    parse_positive_u32(key, &var_or(key, &default.to_string()))
}

fn parse_positive_u32(key: &str, raw: &str) -> Result<NonZeroU32, String> {
    let value: u32 = raw
        .trim()
        .parse()
        .map_err(|e| format!("{key} must be a positive whole number, got {raw:?}: {e}"))?;
    NonZeroU32::new(value).ok_or_else(|| format!("{key} must be greater than zero, got {raw:?}"))
}

fn u32_at_least(key: &str, default: u32, minimum: u32) -> Result<NonZeroU32, String> {
    parse_u32_at_least(key, &var_or(key, &default.to_string()), minimum)
}

fn parse_u32_at_least(key: &str, raw: &str, minimum: u32) -> Result<NonZeroU32, String> {
    let value = parse_positive_u32(key, raw)?;
    if value.get() < minimum {
        return Err(format!(
            "{key} must be at least {minimum} so configured health probes cannot exhaust it, got {}",
            value.get()
        ));
    }
    Ok(value)
}

fn parse_bounded_usize(
    key: &str,
    raw: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let value: usize = raw
        .trim()
        .parse()
        .map_err(|e| format!("{key} must be a whole number, got {raw:?}: {e}"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{key} must be between {minimum} and {maximum}, got {raw:?}"
        ));
    }
    Ok(value)
}

fn parse_bounded_u64(key: &str, raw: &str, minimum: u64, maximum: u64) -> Result<u64, String> {
    let value: u64 = raw
        .parse()
        .map_err(|error| format!("{key} must be a whole number: {error}"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

fn parse_bounded_u32(key: &str, raw: &str, minimum: u32, maximum: u32) -> Result<u32, String> {
    let value: u32 = raw
        .parse()
        .map_err(|error| format!("{key} must be a whole number: {error}"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

fn parse_ip_networks(key: &str, raw: &str) -> Result<Vec<IpNet>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return Err(format!("{key} contains an empty network in {raw:?}"));
            }
            part.parse::<IpNet>()
                .map_err(|e| format!("{key} contains an invalid IP network {part:?}: {e}"))
        })
        .collect()
}

fn validate_cloudflare_proxy_networks(
    gfe_cidrs: &[IpNet],
    cloudflare_cidrs: &[IpNet],
) -> Result<(), String> {
    if cloudflare_cidrs.is_empty() {
        return Ok(());
    }

    let pinned = parse_ip_networks(
        "built-in Cloudflare proxy CIDRs",
        PINNED_CLOUDFLARE_PROXY_CIDRS,
    )?;
    let exact_set = cloudflare_cidrs.len() == pinned.len()
        && cloudflare_cidrs
            .iter()
            .all(|network| pinned.contains(network))
        && cloudflare_cidrs
            .iter()
            .enumerate()
            .all(|(index, network)| !cloudflare_cidrs[..index].contains(network));
    if !exact_set {
        return Err(format!(
            "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS must be empty or exactly the reviewed built-in set of {} networks",
            pinned.len()
        ));
    }

    if gfe_cidrs.iter().any(|gfe| {
        cloudflare_cidrs
            .iter()
            .any(|cloudflare| ip_networks_overlap(gfe, cloudflare))
    }) {
        return Err(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS and CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS must not overlap"
                .to_owned(),
        );
    }
    Ok(())
}

fn ip_networks_overlap(left: &IpNet, right: &IpNet) -> bool {
    match (left, right) {
        (IpNet::V4(left), IpNet::V4(right)) => {
            left.contains(&right.network()) || right.contains(&left.network())
        }
        (IpNet::V6(left), IpNet::V6(right)) => {
            left.contains(&right.network()) || right.contains(&left.network())
        }
        (IpNet::V4(_), IpNet::V6(_)) | (IpNet::V6(_), IpNet::V4(_)) => false,
    }
}

/// Read a number of days as a duration.
fn days(key: &str, default: u64) -> Result<std::time::Duration, String> {
    parse_days(key, &var_or(key, &default.to_string()))
}

/// Turn a written number of days into a duration.
///
/// A value that is not a positive whole number of days is refused rather than
/// silently replaced by the default: a retention set to `"ninety"` or to `0`
/// is somebody trying to say something, and quietly ignoring it would keep or
/// discard records against their intent.
fn parse_days(key: &str, raw: &str) -> Result<std::time::Duration, String> {
    let days: u64 = raw
        .trim()
        .parse()
        .map_err(|e| format!("{key} must be a whole number of days, got {raw:?}: {e}"))?;
    if days == 0 {
        return Err(format!("{key} must be at least one day, got {raw:?}"));
    }
    Ok(std::time::Duration::from_secs(days * 86_400))
}

fn parse_cutover_drain(key: &str, raw: &str) -> Result<bool, String> {
    match raw {
        "" => Ok(false),
        "drained" => Ok(true),
        _ => Err(format!(
            "{key} must be unset or exactly \"drained\" after every incompatible CMA process and its MongoDB and provider operations have stopped"
        )),
    }
}

/// Read a comma-separated list, dropping blank entries.
///
/// Blank entries are dropped rather than kept as empty strings: a trailing
/// comma is the most common way to write one of these by hand, and an empty
/// package name would reach the runtime as an install of nothing.
/// Read a value the core models as a closed set of names.
///
/// Refused rather than defaulted when unrecognised. These decide how freely an
/// agent acts and whose access it spends, and a typo silently becoming the
/// default is exactly the failure that would not be noticed until it mattered.
fn parse_enum<T: serde::de::DeserializeOwned>(key: &str, default: &str) -> Result<T, String> {
    let raw = var_or(key, default);
    let quoted = format!("\"{}\"", raw.trim());
    serde_json::from_str(&quoted).map_err(|_| format!("{key} does not name a value: {raw:?}"))
}

fn list_or(key: &str, default: &str) -> Vec<String> {
    var_or(key, default)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A resolved configuration for tests, with every field set to something
/// plausible.
///
/// Here rather than in a test module because more than one module needs it,
/// and a second copy is a second thing to update when a key is added — which
/// is exactly when a stale copy stops compiling and gets filled in blindly.
#[cfg(test)]
impl Config {
    pub fn sample() -> Self {
        Config {
            bind: "0.0.0.0:8080".into(),
            public_url: "http://x".into(),
            cors_origins: vec![],
            rate_limits: RateLimitConfig::sample(),
            trusted_proxies: TrustedProxyConfig::sample(),
            sandbox_base_url: "http://r".into(),
            sandbox_api_key: "k".into(),
            trigger_sandbox_provider: Some(SandboxProviderConfig {
                base_url: "http://trigger-r".into(),
                api_key: "trigger-k".into(),
                namespace: "trigger-runtimes".into(),
            }),
            sandbox_image: "img".into(),
            sandbox_cpu: "1".into(),
            sandbox_memory: "1Gi".into(),
            default_agent: Default::default(),
            baseline_packages: vec![],
            baseline_skills: vec![],
            baseline_services: vec![],
            deployment_id: DEFAULT_DEPLOYMENT_ID.into(),
            adopts_untagged_runtimes: true,
            upstream_base_url: "http://u".into(),
            upstream_model: "m".into(),
            upstream_wire_api: "responses".into(),
            claude_upstream_base_url: "http://u".into(),
            claude_upstream_model: "m".into(),
            dsh_upstream_base_url: "http://u".into(),
            dsh_upstream_model: "m".into(),
            platform_base_url: "http://p".into(),
            mongodb_uri: "mongodb://h/db".into(),
            mongodb_database: "db".into(),
            mongodb_legacy_cutover: false,
            mongodb_coordination_cutover: false,
            auth_encryption_key: AuthEncryptionKey::from_base64(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .expect("test key has 32 decoded bytes"),
            activity_retention: std::time::Duration::from_secs(90 * 86_400),
            context_bucket: "bkt".into(),
            trigger_workspace_bucket: "trigger-bkt".into(),
            trigger: TriggerRuntimeConfig::sample(),
            sponsorship: None,
            github_capability_signing: None,
            storage_client_id: "sa-id".into(),
            storage_client_secret: "sa-secret".into(),
            auth_mode: AuthMode::Trusted {
                subject: "s".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    const SPONSORSHIP_REQUIRED_CONFIG_KEYS: [&str; 33] = [
        "CHRONO_SPONSORSHIP_EPOCH",
        "CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST",
        "CHRONO_SPONSORSHIP_EXPIRY_EVIDENCE_ID",
        "CHRONO_SPONSORSHIP_REVOCATION_EVIDENCE_ID",
        "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY",
        "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS",
        "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS",
        "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS",
        "CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES",
        "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS",
        "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS",
        "CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR",
        "CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS",
        "CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW",
        "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS",
        "CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE",
        "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
        "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
        "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS",
        "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS",
        "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
        "CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID",
        "CHRONO_SPONSORSHIP_PROVIDER_BASE_URL",
        "CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID",
        "CHRONO_SPONSORSHIP_BILLING_BASE_URL",
        "CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID",
        "CHRONO_SPONSORSHIP_PRICING_BASE_URL",
        "CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS",
        "CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE",
        "CHRONO_SPONSORSHIP_WORKER_MAX_PAGES",
        "CHRONO_SPONSORSHIP_WORKER_CONCURRENCY",
        "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE",
        "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE",
    ];

    struct SponsorshipSecretFiles {
        keyring: PathBuf,
        audit_hmac: PathBuf,
    }

    impl SponsorshipSecretFiles {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let prefix = format!("cma-sponsorship-config-{}-{sequence}", std::process::id());
            let keyring = std::env::temp_dir().join(format!("{prefix}-keyring"));
            let audit_hmac = std::env::temp_dir().join(format!("{prefix}-audit-hmac"));
            std::fs::write(&keyring, "fixture-encryption-keyring\n")
                .expect("the keyring fixture is writable");
            std::fs::write(&audit_hmac, "fixture-audit-hmac-key\n")
                .expect("the audit-HMAC fixture is writable");
            Self {
                keyring,
                audit_hmac,
            }
        }
    }

    impl Drop for SponsorshipSecretFiles {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.keyring);
            let _ = std::fs::remove_file(&self.audit_hmac);
        }
    }

    fn staged_config(values: &[(&str, &str)]) -> Result<Option<MongoCutoverStageConfig>, String> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        MongoCutoverStageConfig::from_lookup(|key| Ok(values.get(key).cloned()))
    }

    fn sponsorship_config(
        values: &BTreeMap<String, String>,
    ) -> Result<Option<SponsorshipRuntimeConfig>, String> {
        SponsorshipRuntimeConfig::from_lookup(|key| Ok(values.get(key).cloned()))
    }

    fn complete_sponsorship_values(
        files: &SponsorshipSecretFiles,
        enabled: bool,
        kill_switch_engaged: bool,
    ) -> BTreeMap<String, String> {
        let allowed_provider_policy_keys = BTreeSet::from([
            "provider-policy-v1".to_owned(),
            "provider-policy-v2".to_owned(),
        ]);
        let gate = SponsorshipGate {
            enabled,
            expiry_evidence_id: Some("expiry-evidence-445".into()),
            revocation_evidence_id: Some("revocation-evidence-524-528".into()),
            allowed_provider_policy_keys,
            maximum_policy_budget_micros: 1_000_000,
            maximum_policy_requests: 500,
            maximum_concurrent_launches: 10,
            kill_switch_engaged,
        };
        BTreeMap::from([
            ("CHRONO_SPONSORSHIP_ENABLED".into(), enabled.to_string()),
            (
                "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED".into(),
                kill_switch_engaged.to_string(),
            ),
            ("CHRONO_SPONSORSHIP_EPOCH".into(), "1".into()),
            (
                "CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST".into(),
                gate.configuration_digest().expect("the test gate digests"),
            ),
            (
                "CHRONO_SPONSORSHIP_EXPIRY_EVIDENCE_ID".into(),
                "expiry-evidence-445".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_REVOCATION_EVIDENCE_ID".into(),
                "revocation-evidence-524-528".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY".into(),
                "provider-policy-v1".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS".into(),
                "provider-policy-v1,provider-policy-v2".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS".into(),
                "1000000".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS".into(),
                "500".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES".into(),
                "10".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS".into(),
                "900".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS".into(),
                "3600".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR".into(),
                "10".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS".into(),
                "60".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW".into(),
                "20".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS".into(),
                "100".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE".into(),
                "60".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS".into(),
                "100000".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS".into(),
                "50000".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS".into(),
                "500000".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS".into(),
                "500000".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS".into(),
                "100000".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID".into(),
                "provider-adapter-v1".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_PROVIDER_BASE_URL".into(),
                "https://provider.sponsorship.test".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID".into(),
                "billing-adapter-v1".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_BILLING_BASE_URL".into(),
                "https://billing.sponsorship.test".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID".into(),
                "pricing-adapter-v1".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_PRICING_BASE_URL".into(),
                "https://pricing.sponsorship.test".into(),
            ),
            (
                "CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS".into(),
                "5".into(),
            ),
            ("CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE".into(), "50".into()),
            ("CHRONO_SPONSORSHIP_WORKER_MAX_PAGES".into(), "20".into()),
            ("CHRONO_SPONSORSHIP_WORKER_CONCURRENCY".into(), "4".into()),
            (
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE".into(),
                files.keyring.to_string_lossy().into_owned(),
            ),
            (
                "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE".into(),
                files.audit_hmac.to_string_lossy().into_owned(),
            ),
        ])
    }

    #[test]
    fn sponsorship_has_one_exact_dormant_default() {
        assert!(sponsorship_config(&BTreeMap::new())
            .expect("the undeclared deployment uses closed defaults")
            .is_none());

        let mut declared = BTreeMap::from([
            ("CHRONO_SPONSORSHIP_ENABLED".into(), "false".into()),
            (
                "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED".into(),
                "true".into(),
            ),
        ]);
        for key in SPONSORSHIP_REQUIRED_CONFIG_KEYS {
            declared.insert(key.into(), String::new());
        }
        assert!(sponsorship_config(&declared)
            .expect("the fixed ConfigMap's empty staging values stay dormant")
            .is_none());
    }

    #[test]
    fn a_complete_tuple_can_be_staged_closed_but_never_enabled_in_this_build() {
        let files = SponsorshipSecretFiles::new();
        let staged = sponsorship_config(&complete_sponsorship_values(&files, false, true))
            .expect("the complete closed tuple is valid staging")
            .expect("staging is represented explicitly");
        assert!(!staged.gate.enabled);
        assert!(staged.gate.kill_switch_engaged);
        assert!(staged.fence.kill_switch_engaged);
        assert_eq!(staged.trusted_provider_policy_key, "provider-policy-v1");
        assert_eq!(staged.limits.default_policy_expiry.as_secs(), 900);
        assert_eq!(staged.limits.maximum_requests_per_minute, 60);
        assert_eq!(staged.adapters.provider_adapter_id, "provider-adapter-v1");
        assert_eq!(staged.worker.page_size, 50);

        for values in [
            BTreeMap::from([("CHRONO_SPONSORSHIP_ENABLED".into(), "true".into())]),
            complete_sponsorship_values(&files, true, false),
            complete_sponsorship_values(&files, true, true),
        ] {
            assert_eq!(
                sponsorship_config(&values).expect_err(
                    "enabled sponsorship must fail until runtime composition is implemented"
                ),
                SPONSORSHIP_NOT_COMPOSED_ERROR
            );
        }
    }

    #[test]
    fn enabled_and_partially_staged_disabled_tuples_fail_startup() {
        let files = SponsorshipSecretFiles::new();
        let incomplete = BTreeMap::from([
            ("CHRONO_SPONSORSHIP_ENABLED".into(), "true".into()),
            (
                "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED".into(),
                "false".into(),
            ),
        ]);
        assert_eq!(
            sponsorship_config(&incomplete).expect_err("enabled configuration is unavailable"),
            SPONSORSHIP_NOT_COMPOSED_ERROR
        );

        let complete = complete_sponsorship_values(&files, false, true);
        for missing in SPONSORSHIP_REQUIRED_CONFIG_KEYS {
            let mut values = complete.clone();
            values.remove(missing);
            let error = sponsorship_config(&values)
                .expect_err("every public gate, adapter, limit, worker, and file key is required");
            assert!(
                error.contains(missing),
                "missing {missing} produced an unrelated error: {error}"
            );
        }

        for lone_key in SPONSORSHIP_REQUIRED_CONFIG_KEYS {
            let mut values = BTreeMap::from([
                ("CHRONO_SPONSORSHIP_ENABLED".into(), "false".into()),
                (
                    "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED".into(),
                    "true".into(),
                ),
            ]);
            values.insert(lone_key.into(), complete[lone_key].clone());
            assert!(
                sponsorship_config(&values).is_err(),
                "a lone staging value was accepted: {lone_key}"
            );
        }
    }

    #[test]
    fn disabled_sponsorship_staging_requires_the_kill_switch() {
        let error = sponsorship_config(&BTreeMap::from([(
            "CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED".into(),
            "false".into(),
        )]))
        .expect_err("a dormant deployment cannot disengage the switch");
        assert!(error.contains("CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED"));

        for (key, value) in [
            ("CHRONO_SPONSORSHIP_ENABLED", "TRUE"),
            ("CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED", "yes"),
        ] {
            let error = sponsorship_config(&BTreeMap::from([(key.into(), value.into())]))
                .expect_err("sponsorship booleans use one exact spelling");
            assert!(error.contains(key), "{error}");
        }
    }

    #[test]
    fn sponsorship_epoch_and_digest_are_strict_fences() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        for epoch in ["0", "-1", "9223372036854775808"] {
            let mut values = complete.clone();
            values.insert("CHRONO_SPONSORSHIP_EPOCH".into(), epoch.into());
            let error = sponsorship_config(&values).expect_err("an unusable epoch must fail");
            assert!(error.contains("CHRONO_SPONSORSHIP_EPOCH"), "{error}");
        }
        for digest in ["not-a-digest", &"0".repeat(64)] {
            let mut values = complete.clone();
            values.insert(
                "CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST".into(),
                digest.into(),
            );
            let error = sponsorship_config(&values)
                .expect_err("a malformed or mismatched digest must fail");
            assert!(
                error.contains("CHRONO_SPONSORSHIP_CONFIGURATION_DIGEST"),
                "{error}"
            );
        }
    }

    #[test]
    fn sponsorship_evidence_and_policy_keys_are_canonical_and_explicit() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        for (key, value) in [
            ("CHRONO_SPONSORSHIP_EXPIRY_EVIDENCE_ID", "short"),
            (
                "CHRONO_SPONSORSHIP_REVOCATION_EVIDENCE_ID",
                "revocation evidence with spaces",
            ),
            ("CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS", ""),
            (
                "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS",
                "provider-policy-v1,provider-policy-v1",
            ),
            (
                "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS",
                "provider-policy-v1, provider-policy-v2",
            ),
            (
                "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY",
                "provider policy v1",
            ),
        ] {
            let mut values = complete.clone();
            values.insert(key.into(), value.into());
            assert!(
                sponsorship_config(&values).is_err(),
                "accepted noncanonical {key}={value:?}"
            );
        }

        let mut untrusted = complete;
        untrusted.insert(
            "CHRONO_SPONSORSHIP_TRUSTED_PROVIDER_POLICY_KEY".into(),
            "provider-policy-v3".into(),
        );
        let error = sponsorship_config(&untrusted)
            .expect_err("the selected policy must be deployment-allowlisted");
        assert!(error.contains("ALLOWED_PROVIDER_POLICY_KEYS"));

        let files = SponsorshipSecretFiles::new();
        let mut too_many = complete_sponsorship_values(&files, false, true);
        too_many.insert(
            "CHRONO_SPONSORSHIP_ALLOWED_PROVIDER_POLICY_KEYS".into(),
            (0..=MAX_SPONSORSHIP_PROVIDER_POLICY_KEYS)
                .map(|index| format!("provider-policy-{index}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        let error = sponsorship_config(&too_many)
            .expect_err("the provider-policy allowlist has a fixed cardinality bound");
        assert!(error.contains("at most 32"), "{error}");
    }

    #[test]
    fn sponsorship_expiry_is_absolutely_bounded_and_ordered() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        for (key, value) in [
            ("CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS", "59"),
            ("CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS", "86401"),
            ("CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS", "59"),
            ("CHRONO_SPONSORSHIP_MAXIMUM_POLICY_EXPIRY_SECONDS", "86401"),
        ] {
            let mut values = complete.clone();
            values.insert(key.into(), value.into());
            let error = sponsorship_config(&values).expect_err("expiry outside the bound fails");
            assert!(error.contains(key), "{error}");
        }

        let mut reversed = complete;
        reversed.insert(
            "CHRONO_SPONSORSHIP_DEFAULT_POLICY_EXPIRY_SECONDS".into(),
            "3601".into(),
        );
        let error = sponsorship_config(&reversed)
            .expect_err("the default cannot outlive the deployment maximum");
        assert!(error.contains("must not exceed"));
    }

    #[test]
    fn every_sponsorship_resource_and_worker_cap_has_a_hard_ceiling() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        let caps = [
            ("CHRONO_SPONSORSHIP_EPOCH", "9223372036854775808"),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_BUDGET_MICROS",
                "100000001",
            ),
            ("CHRONO_SPONSORSHIP_MAXIMUM_POLICY_REQUESTS", "10001"),
            ("CHRONO_SPONSORSHIP_MAXIMUM_CONCURRENT_LAUNCHES", "33"),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_ACTIVE_POLICIES_PER_SPONSOR",
                "33",
            ),
            ("CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS", "3601"),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_LAUNCHES_PER_SPONSOR_PER_WINDOW",
                "101",
            ),
            ("CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS", "1001"),
            ("CHRONO_SPONSORSHIP_MAXIMUM_REQUESTS_PER_MINUTE", "601"),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
                "1000001",
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
                "1000001",
            ),
            ("CHRONO_SPONSORSHIP_MAXIMUM_POLICY_INPUT_TOKENS", "10000001"),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_POLICY_OUTPUT_TOKENS",
                "10000001",
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
                "10000001",
            ),
            ("CHRONO_SPONSORSHIP_WORKER_INTERVAL_SECONDS", "301"),
            ("CHRONO_SPONSORSHIP_WORKER_PAGE_SIZE", "201"),
            ("CHRONO_SPONSORSHIP_WORKER_MAX_PAGES", "101"),
            ("CHRONO_SPONSORSHIP_WORKER_CONCURRENCY", "33"),
        ];
        for (key, above_ceiling) in caps {
            for value in ["0", above_ceiling] {
                let mut values = complete.clone();
                values.insert(key.into(), value.into());
                let error = sponsorship_config(&values)
                    .expect_err("zero and above-ceiling limits must fail startup");
                assert!(error.contains(key), "{key}={value} produced {error}");
            }
        }
        let mut too_short = complete;
        too_short.insert(
            "CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS".into(),
            "59".into(),
        );
        let error = sponsorship_config(&too_short)
            .expect_err("the public Launch abuse window is at least one minute");
        assert!(
            error.contains("CHRONO_SPONSORSHIP_LAUNCH_RATE_WINDOW_SECONDS"),
            "{error}"
        );
    }

    #[test]
    fn per_launch_caps_cannot_exceed_policy_caps() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        for (key, value) in [
            ("CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_REQUESTS", "501"),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_INPUT_TOKENS",
                "500001",
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_OUTPUT_TOKENS",
                "500001",
            ),
            (
                "CHRONO_SPONSORSHIP_MAXIMUM_PER_LAUNCH_BUDGET_MICROS",
                "1000001",
            ),
        ] {
            let mut values = complete.clone();
            values.insert(key.into(), value.into());
            let error = sponsorship_config(&values)
                .expect_err("a per-Launch ceiling cannot exceed its policy ceiling");
            assert!(error.contains(key), "{error}");
            assert!(error.contains("must not exceed"), "{error}");
        }
    }

    #[test]
    fn sponsorship_adapter_identities_and_origins_are_canonical() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        for key in [
            "CHRONO_SPONSORSHIP_PROVIDER_ADAPTER_ID",
            "CHRONO_SPONSORSHIP_BILLING_ADAPTER_ID",
            "CHRONO_SPONSORSHIP_PRICING_ADAPTER_ID",
        ] {
            for value in ["adapter with spaces", "adapter?query", " adapter"] {
                let mut values = complete.clone();
                values.insert(key.into(), value.into());
                assert!(sponsorship_config(&values).is_err(), "{key}={value:?}");
            }
        }
        for key in [
            "CHRONO_SPONSORSHIP_PROVIDER_BASE_URL",
            "CHRONO_SPONSORSHIP_BILLING_BASE_URL",
            "CHRONO_SPONSORSHIP_PRICING_BASE_URL",
        ] {
            for value in [
                "http://adapter.test",
                "https://Adapter.test",
                "https://adapter.test/",
                "https://user:password@adapter.test",
                "https://adapter.test/v1",
                "https://adapter.test?tenant=other",
            ] {
                let mut values = complete.clone();
                values.insert(key.into(), value.into());
                let error = sponsorship_config(&values)
                    .expect_err("noncanonical adapter origins must fail startup");
                assert!(error.contains(key), "{key}={value:?} produced {error}");
            }
        }
    }

    #[test]
    fn sponsorship_secrets_are_file_only_nonempty_distinct_and_redacted() {
        let files = SponsorshipSecretFiles::new();
        let complete = complete_sponsorship_values(&files, false, true);
        for (inline_key, fixture) in [
            (
                "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING",
                "inline-keyring-must-not-leak",
            ),
            (
                "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY",
                "inline-audit-key-must-not-leak",
            ),
        ] {
            let mut values = complete.clone();
            values.insert(inline_key.into(), fixture.into());
            let error = sponsorship_config(&values)
                .expect_err("inline sponsorship secrets are never accepted");
            assert!(error.contains(inline_key), "{error}");
            assert!(!error.contains(fixture), "{error}");
        }

        let mut aliased = complete.clone();
        aliased.insert(
            "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE".into(),
            files.keyring.to_string_lossy().into_owned(),
        );
        assert!(sponsorship_config(&aliased)
            .expect_err("the two secret classes cannot alias one file")
            .contains("separate mounted files"));

        std::fs::write(&files.audit_hmac, "fixture-encryption-keyring\n")
            .expect("the duplicate-material fixture is writable");
        assert!(sponsorship_config(&complete)
            .expect_err("the two files cannot reuse one secret")
            .contains("independent secret material"));

        std::fs::write(&files.audit_hmac, "fixture-audit-hmac-key\n")
            .expect("the audit fixture is restored");
        std::fs::write(&files.keyring, " \n").expect("the empty fixture is writable");
        let error =
            sponsorship_config(&complete).expect_err("an empty sponsorship keyring fails closed");
        assert!(error.contains("CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE"));

        std::fs::write(&files.keyring, vec![b'x'; MAX_FILE_SECRET_BYTES + 1])
            .expect("the oversized fixture is writable");
        let error = sponsorship_config(&complete)
            .expect_err("an oversized sponsorship keyring fails closed");
        assert!(error.contains("secret-file limit"), "{error}");

        std::fs::write(&files.keyring, "fixture-encryption-keyring\n")
            .expect("the keyring fixture is restored");
        let config = sponsorship_config(&complete)
            .expect("the restored secret tuple is valid")
            .expect("the runtime is configured");
        let debug = format!("{config:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("fixture-encryption-keyring"));
        assert!(!debug.contains("fixture-audit-hmac-key"));
    }

    #[test]
    fn trigger_runtime_capabilities_accept_only_the_six_canonical_names() {
        let raw = "conversation,durable_conversation,workspace_checkpoint,restricted_service_gateway,credential_refresh,credential_revocation";
        assert_eq!(
            parse_trigger_runtime_capabilities("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", raw)
                .expect("the complete canonical inventory is usable"),
            all_trigger_runtime_capabilities()
        );
        assert_eq!(
            parse_trigger_runtime_capabilities("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", "")
                .expect("an empty declaration deliberately admits no Trigger capability"),
            Vec::<RequiredCapability>::new()
        );
    }

    #[test]
    fn trigger_runtime_capabilities_reject_duplicates_and_noncanonical_spelling() {
        for raw in [
            "conversation,conversation",
            " conversation",
            "conversation ",
            "conversation, durable_conversation",
            "Conversation",
            "unknown_capability",
        ] {
            let error =
                parse_trigger_runtime_capabilities("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", raw)
                    .expect_err("ambiguous or noncanonical capability input must fail startup");
            assert!(
                error.contains("CHRONO_TRIGGER_RUNTIME_CAPABILITIES"),
                "the startup error must name its deployment key: {error}"
            );
        }
    }

    #[test]
    fn trigger_public_github_observer_slug_is_optional_and_canonical() {
        assert_eq!(
            parse_optional_service_slug("CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG", "api-github")
                .unwrap()
                .as_deref(),
            Some("api-github")
        );
        assert_eq!(
            parse_optional_service_slug("CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG", "").unwrap(),
            None
        );
        for invalid in ["API-GitHub", "api/github", "-api-github", "api-github-"] {
            assert!(
                parse_optional_service_slug("CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG", invalid)
                    .is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn a_blank_value_falls_back_to_the_default() {
        // An env var set to the empty string is a common deployment mistake and
        // must behave as absent rather than producing an empty bind address.
        assert_eq!(var_or("CHRONO_TEST_ABSENT_KEY", "fallback"), "fallback");
    }

    #[test]
    fn a_new_deployment_defaults_to_the_cma_identity() {
        assert_eq!(
            var_or("CHRONO_TEST_ABSENT_DEPLOYMENT_ID", DEFAULT_DEPLOYMENT_ID),
            "cma"
        );
    }

    #[test]
    fn a_new_deployment_defaults_to_the_cma_runtime_image() {
        assert_eq!(
            var_or("CHRONO_TEST_ABSENT_SANDBOX_IMAGE", DEFAULT_SANDBOX_IMAGE),
            "cma-runtime:dev"
        );
    }

    #[test]
    fn the_baseline_reaches_the_skill_runtime_and_not_only_its_registry() {
        // `ornn` and `ornn-api` are two services. The registry is what this
        // service searches to show a person what they may pick; the runtime is
        // what the agent calls once it has picked. A baseline naming only the
        // registry mints a credential the provider refuses on every call the
        // agent actually makes — the failure this pins.
        let baseline = list_or("CHRONO_TEST_ABSENT_SERVICES", BASELINE_SERVICES);
        assert!(
            baseline.iter().any(|s| s == "ornn"),
            "the skill runtime must be reachable, got {baseline:?}"
        );
        assert!(
            baseline.iter().any(|s| s == "ornn-api"),
            "the skill registry must stay reachable, got {baseline:?}"
        );
    }

    #[test]
    fn require_names_the_missing_key() {
        let err = require("CHRONO_TEST_DEFINITELY_ABSENT").expect_err("must fail");
        assert!(err.contains("CHRONO_TEST_DEFINITELY_ABSENT"), "got {err}");
    }

    #[test]
    fn a_retention_falls_back_to_its_default_in_whole_days() {
        let d = days("CHRONO_TEST_ABSENT_RETENTION", 90).expect("the default is usable");
        assert_eq!(d.as_secs(), 90 * 86_400);
        assert_eq!(
            parse_days("CHRONO_ACTIVITY_RETENTION_DAYS", " 7 ")
                .expect("whitespace is not a value")
                .as_secs(),
            7 * 86_400
        );
    }

    #[test]
    fn a_retention_that_is_not_a_number_of_days_is_refused_rather_than_defaulted() {
        // Quietly ignoring it would keep records for a length of time nobody
        // chose, which is the failure mode this exists to prevent.
        for raw in ["ninety", "-1", "90d", ""] {
            let err = parse_days("CHRONO_ACTIVITY_RETENTION_DAYS", raw)
                .expect_err("{raw:?} is not a number of days");
            assert!(err.contains("CHRONO_ACTIVITY_RETENTION_DAYS"), "got {err}");
        }
    }

    #[test]
    fn a_retention_of_no_days_is_refused_rather_than_deleting_everything() {
        let err =
            parse_days("CHRONO_ACTIVITY_RETENTION_DAYS", "0").expect_err("zero is not a retention");
        assert!(err.contains("at least one day"), "got {err}");
    }

    #[test]
    fn request_rates_accept_only_positive_whole_numbers() {
        assert_eq!(
            parse_positive_u32("CHRONO_SERVER_RATE_CREATE_PER_MINUTE", " 17 ")
                .expect("a positive rate is usable")
                .get(),
            17
        );
        for raw in ["0", "-1", "1.5", "many", ""] {
            let error = parse_positive_u32("CHRONO_SERVER_RATE_CREATE_PER_MINUTE", raw)
                .expect_err("an unusable rate must fail startup");
            assert!(
                error.contains("CHRONO_SERVER_RATE_CREATE_PER_MINUTE"),
                "got {error}"
            );
        }
    }

    #[test]
    fn health_rate_cannot_starve_the_declared_probe_schedule() {
        assert_eq!(
            u32_at_least(
                "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
                HEALTH_RATE_REQUESTS_PER_MINUTE,
                MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE,
            )
            .unwrap()
            .get(),
            HEALTH_RATE_REQUESTS_PER_MINUTE
        );
        let error = parse_u32_at_least(
            "CHRONO_SERVER_RATE_HEALTH_PER_MINUTE",
            "29",
            MINIMUM_HEALTH_RATE_REQUESTS_PER_MINUTE,
        )
        .expect_err("a probe-starving budget must be refused");
        assert!(error.contains("at least 30"), "got {error}");
    }

    #[test]
    fn trusted_proxy_configuration_is_explicit_and_bounded() {
        let networks = parse_ip_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            " 35.191.0.0/16, 2600:2d00:1:1::/64 ",
        )
        .expect("documented GFE networks parse");
        assert_eq!(networks.len(), 2);
        assert!(networks[0].contains(&"35.191.4.2".parse::<std::net::IpAddr>().unwrap()));
        assert!(networks[1].contains(&"2600:2d00:1:1::9".parse::<std::net::IpAddr>().unwrap()));
        assert!(parse_ip_networks("CIDRS", "").unwrap().is_empty());
        for raw in [",", "35.191.0.0/16,", "not-a-network"] {
            assert!(
                parse_ip_networks("CHRONO_SERVER_TRUSTED_PROXY_CIDRS", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_bounded_usize("CHRONO_SERVER_TRUSTED_PROXY_HOPS", "1", 1, 16).unwrap(),
            1
        );
        for raw in ["0", "17", "many"] {
            assert!(
                parse_bounded_usize("CHRONO_SERVER_TRUSTED_PROXY_HOPS", raw, 1, 16).is_err(),
                "accepted {raw:?}"
            );
        }

        let cloudflare = parse_ip_networks(
            "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS",
            PINNED_CLOUDFLARE_PROXY_CIDRS,
        )
        .expect("reviewed Cloudflare networks parse");
        assert_eq!(cloudflare.len(), 22);
        assert_eq!(
            cloudflare
                .iter()
                .filter(|network| matches!(network, IpNet::V4(_)))
                .count(),
            15
        );
        assert_eq!(
            cloudflare
                .iter()
                .filter(|network| matches!(network, IpNet::V6(_)))
                .count(),
            7
        );
        assert!(cloudflare[0].contains(&"173.245.48.9".parse::<std::net::IpAddr>().unwrap()));
        assert!(cloudflare[16].contains(&"2606:4700::9".parse::<std::net::IpAddr>().unwrap()));
        for raw in [",", "173.245.48.0/20,", "not-a-network"] {
            assert!(
                parse_ip_networks("CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        let gfe = parse_ip_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            "35.191.0.0/16,130.211.0.0/22,2600:2d00:1:1::/64",
        )
        .unwrap();
        validate_cloudflare_proxy_networks(&gfe, &cloudflare)
            .expect("the exact reviewed sets do not overlap");
        validate_cloudflare_proxy_networks(&gfe, &[])
            .expect("empty Cloudflare trust disables the conditional branch");

        let mut partial = cloudflare.clone();
        partial.pop();
        let error = validate_cloudflare_proxy_networks(&gfe, &partial)
            .expect_err("a partial production pin must not start");
        assert!(error.contains("exactly the reviewed built-in set"));

        let mut duplicated = cloudflare.clone();
        duplicated[0] = duplicated[1];
        let error = validate_cloudflare_proxy_networks(&gfe, &duplicated)
            .expect_err("a duplicated range hides a missing pin");
        assert!(error.contains("exactly the reviewed built-in set"));

        let broadened = parse_ip_networks(
            "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS",
            "0.0.0.0/0,2400:cb00::/32",
        )
        .unwrap();
        let error = validate_cloudflare_proxy_networks(&gfe, &broadened)
            .expect_err("an operator-supplied supernet must not become authority");
        assert!(error.contains("exactly the reviewed built-in set"));

        let overlapping_gfe = parse_ip_networks(
            "CHRONO_SERVER_TRUSTED_PROXY_CIDRS",
            "35.191.0.0/16,173.245.48.0/20",
        )
        .unwrap();
        let error = validate_cloudflare_proxy_networks(&overlapping_gfe, &cloudflare)
            .expect_err("proxy authority sets must remain disjoint");
        assert!(error.contains("must not overlap"));
    }

    #[test]
    fn cutover_gates_require_the_exact_post_drain_acknowledgement() {
        assert!(!parse_cutover_drain("CUTOVER", "").unwrap());
        assert!(parse_cutover_drain("CUTOVER", "drained").unwrap());
        for unsafe_value in ["true", "yes", "drain", "DRAINED", " drained", "drained "] {
            let error = parse_cutover_drain("CUTOVER", unsafe_value)
                .expect_err("an imprecise acknowledgement must stay closed");
            assert!(error.contains("exactly \"drained\""), "got {error}");
        }
    }

    #[test]
    fn trigger_rollout_values_fail_closed_instead_of_guessing() {
        assert!(strict_bool("TRIGGER_BOOL", "true").unwrap());
        assert!(!strict_bool("TRIGGER_BOOL", "false").unwrap());
        for raw in ["TRUE", "False", "1", "yes", " true", ""] {
            assert!(
                strict_bool("TRIGGER_BOOL", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_u64_list("CHRONO_TRIGGER_REPOSITORY_IDS", " 17,42 ").unwrap(),
            vec![17, 42]
        );
        assert!(parse_u64_list("CHRONO_TRIGGER_REPOSITORY_IDS", "")
            .unwrap()
            .is_empty());
        for raw in ["0", "17,", "-1", "repo"] {
            assert!(
                parse_u64_list("CHRONO_TRIGGER_REPOSITORY_IDS", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_trigger_cleanup_mode("CHRONO_TRIGGER_CLEANUP_MODE", "dry_run").unwrap(),
            TriggerCleanupMode::DryRun
        );
        assert_eq!(
            parse_trigger_cleanup_mode("CHRONO_TRIGGER_CLEANUP_MODE", "execute").unwrap(),
            TriggerCleanupMode::Execute
        );
        for raw in ["delete", "DRY_RUN", " dry_run", ""] {
            assert!(
                parse_trigger_cleanup_mode("CHRONO_TRIGGER_CLEANUP_MODE", raw).is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_bounded_usize(
                "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
                "1000",
                1,
                layer2_core::MAX_DIFF_SUMMARY_FILES,
            )
            .unwrap(),
            1_000
        );
        for raw in ["0", "1001", "unbounded"] {
            assert!(
                parse_bounded_usize(
                    "CHRONO_TRIGGER_CHANGED_FILE_LIMIT",
                    raw,
                    1,
                    layer2_core::MAX_DIFF_SUMMARY_FILES,
                )
                .is_err(),
                "accepted {raw:?}"
            );
        }

        assert_eq!(
            parse_bounded_usize(
                "CHRONO_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS",
                "300",
                1,
                MAX_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS,
            )
            .unwrap(),
            DEFAULT_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS
        );
        for raw in ["0", "3601", "five minutes"] {
            assert!(
                parse_bounded_usize(
                    "CHRONO_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS",
                    raw,
                    1,
                    MAX_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS,
                )
                .is_err(),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn trigger_provider_lane_is_complete_authority_or_absent() {
        assert!(resolve_trigger_sandbox_provider(
            false,
            "http://ordinary-provider",
            "ordinary-secret",
            None,
            None,
            None,
        )
        .expect("a disabled, undeclared lane is valid")
        .is_none());
        let missing = resolve_trigger_sandbox_provider(
            true,
            "http://ordinary-provider",
            "ordinary-secret",
            Some("http://trigger-provider".into()),
            None,
            Some("secret".into()),
        )
        .expect_err("a provider without a reclamation namespace must fail closed");
        assert!(missing.contains("CHRONO_TRIGGER_SANDBOX_NAMESPACE"));

        let provider = resolve_trigger_sandbox_provider(
            true,
            "http://ordinary-provider",
            "ordinary-secret",
            Some("http://trigger-provider".into()),
            Some("chrono-bot-trigger".into()),
            Some("secret".into()),
        )
        .expect("the complete lane is valid")
        .expect("the lane is present");
        assert_eq!(provider.base_url, "http://trigger-provider");
        assert_eq!(provider.namespace, "chrono-bot-trigger");
        assert_eq!(provider.api_key, "secret");
        assert!(!format!("{provider:?}").contains("secret"));

        for namespace in ["", "Trigger", "two.parts", "-prefix", "suffix-"] {
            assert!(resolve_trigger_sandbox_provider(
                true,
                "http://ordinary-provider",
                "ordinary-secret",
                Some("http://trigger-provider".into()),
                Some(namespace.into()),
                Some("secret".into()),
            )
            .is_err());
        }
        for base_url in [
            "trigger-provider",
            "ftp://trigger-provider",
            "http://user:password@trigger-provider",
            "http://trigger-provider/v1",
            "http://trigger-provider?lane=kata",
        ] {
            assert!(resolve_trigger_sandbox_provider(
                true,
                "http://ordinary-provider",
                "ordinary-secret",
                Some(base_url.into()),
                Some("chrono-bot-trigger".into()),
                Some("secret".into()),
            )
            .is_err());
        }

        for (ordinary_origin, trigger_origin) in [
            ("http://same-provider", "http://same-provider"),
            ("http://same-provider/v1", "http://same-provider"),
            ("https://SAME-provider:443/api", "https://same-provider"),
        ] {
            let error = resolve_trigger_sandbox_provider(
                true,
                ordinary_origin,
                "ordinary-secret",
                Some(trigger_origin.into()),
                Some("chrono-bot-trigger".into()),
                Some("trigger-secret".into()),
            )
            .expect_err("one origin cannot back both provider authorities");
            assert!(error.contains("ordinary sandbox provider origin"));
        }
        let error = resolve_trigger_sandbox_provider(
            true,
            "http://ordinary-provider",
            "reused-secret",
            Some("http://trigger-provider".into()),
            Some("chrono-bot-trigger".into()),
            Some("reused-secret".into()),
        )
        .expect_err("one credential cannot back both provider authorities");
        assert!(error.contains("must not reuse"));
        assert!(!error.contains("reused-secret"));
    }

    #[test]
    fn trigger_provider_secret_is_file_only_nonempty_and_unambiguous() {
        static FILE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = FILE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cma-trigger-provider-secret-{}-{sequence}",
            std::process::id()
        ));

        std::fs::write(&path, " \n").expect("the empty secret fixture is written");
        let error = optional_file_secret_from_values(
            "TRIGGER_FILE",
            "TRIGGER_INLINE",
            Some(path.to_string_lossy().into_owned()),
            None,
        )
        .expect_err("an empty mounted secret fails closed");
        assert!(error.contains("TRIGGER_FILE"));

        std::fs::write(&path, "mounted-secret\n").expect("the secret fixture is replaced");
        let error = optional_file_secret_from_values(
            "TRIGGER_FILE",
            "TRIGGER_INLINE",
            Some(path.to_string_lossy().into_owned()),
            Some("inline-secret".into()),
        )
        .expect_err("an inline value cannot coexist with mounted custody");
        assert!(error.contains("TRIGGER_INLINE"));
        assert!(!error.contains("inline-secret"));

        let resolved = optional_file_secret_from_values(
            "TRIGGER_FILE",
            "TRIGGER_INLINE",
            Some(path.to_string_lossy().into_owned()),
            None,
        )
        .expect("the mounted secret is readable");
        assert_eq!(resolved.as_deref(), Some("mounted-secret"));

        std::fs::remove_file(path).expect("the secret fixture is removed");
        assert!(optional_file_secret_from_values(
            "TRIGGER_FILE",
            "TRIGGER_INLINE",
            None,
            Some(" inline-secret ".into()),
        )
        .expect_err("inline provider credentials are never accepted")
        .contains("TRIGGER_INLINE"));
        assert!(
            optional_file_secret_from_values("TRIGGER_FILE", "TRIGGER_INLINE", None, None)
                .expect("an undeclared secret is absent")
                .is_none()
        );
    }

    #[test]
    fn the_legacy_cutover_stage_accepts_only_its_exact_flag_tuple() {
        let config = staged_config(&[
            (MONGODB_CUTOVER_STAGE, "legacy"),
            ("CHRONO_MONGODB_LEGACY_CUTOVER", "drained"),
            ("CHRONO_MONGODB_COORDINATION_CUTOVER", ""),
            ("CHRONO_MONGODB_URI", "mongodb://stage.invalid/cma"),
        ])
        .expect("the exact staged tuple is usable")
        .expect("the legacy stage is selected");
        assert_eq!(config.stage, MongoCutoverStage::Legacy);
        assert_eq!(config.bind, "0.0.0.0:8080");
        assert_eq!(config.mongodb_database, "chrono_bot");

        for stage in ["unknown", " Legacy", "legacy ", "LEGACY"] {
            let error = staged_config(&[(MONGODB_CUTOVER_STAGE, stage)])
                .expect_err("an unknown stage must fail before other configuration");
            assert!(error.contains(MONGODB_CUTOVER_STAGE), "got {error}");
        }
        let secret_stage = "mongodb://operator:do-not-log-this@invalid";
        let error = staged_config(&[(MONGODB_CUTOVER_STAGE, secret_stage)])
            .expect_err("an invalid stage is refused without echoing its value");
        assert!(!error.contains("do-not-log-this"), "got {error}");
        for legacy in ["", "drain", " drained", "drained ", "DRAINED"] {
            let error = staged_config(&[
                (MONGODB_CUTOVER_STAGE, "legacy"),
                ("CHRONO_MONGODB_LEGACY_CUTOVER", legacy),
            ])
            .expect_err("the stage requires an exact legacy drain acknowledgement");
            assert!(
                error.contains("CHRONO_MONGODB_LEGACY_CUTOVER=drained"),
                "got {error}"
            );
        }
        for coordination in ["drained", " ", "false"] {
            let error = staged_config(&[
                (MONGODB_CUTOVER_STAGE, "legacy"),
                ("CHRONO_MONGODB_LEGACY_CUTOVER", "drained"),
                ("CHRONO_MONGODB_COORDINATION_CUTOVER", coordination),
            ])
            .expect_err("coordination must remain structurally disabled in this stage");
            assert!(
                error.contains("CHRONO_MONGODB_COORDINATION_CUTOVER"),
                "got {error}"
            );
        }
    }

    #[test]
    fn every_trigger_runtime_key_is_pinned_in_the_fixed_configmap() {
        let manifest = include_str!("../../../../infra/api/configmap.yaml");
        for (key, value) in [
            ("CHRONO_TRIGGER_ENABLED", "false"),
            ("CHRONO_TRIGGER_ORCHESTRATION_PAUSED", "true"),
            ("CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH", "true"),
            ("CHRONO_TRIGGER_RUNTIME_CAPABILITIES", ""),
            ("CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG", ""),
            ("CHRONO_TRIGGER_SANDBOX_BASE_URL", ""),
            ("CHRONO_TRIGGER_SANDBOX_NAMESPACE", ""),
            ("CHRONO_TRIGGER_SANDBOX_API_KEY_FILE", ""),
            ("CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS", ""),
            ("CHRONO_TRIGGER_REPOSITORY_IDS", ""),
            ("CHRONO_TRIGGER_USER_COHORT_PERCENT", "0"),
            ("CHRONO_TRIGGER_CLEANUP_MODE", "dry_run"),
            ("CHRONO_TRIGGER_CHANGED_FILE_LIMIT", "1000"),
            ("CHRONO_TRIGGER_FORK_READINESS_TIMEOUT_SECONDS", "300"),
            ("CHRONO_TRIGGER_WORKER_INTERVAL_SECONDS", "5"),
            ("CHRONO_TRIGGER_WORKER_PAGE_SIZE", "50"),
            ("CHRONO_TRIGGER_WORKER_MAX_PAGES", "20"),
            ("CHRONO_TRIGGER_WORKER_CONCURRENCY", "4"),
        ] {
            let declaration = format!("  {key}: \"{value}\"");
            assert!(
                manifest.lines().any(|line| line == declaration),
                "fixed ConfigMap is missing {declaration}"
            );
        }
    }

    #[test]
    fn every_sponsorship_key_is_pinned_to_the_exact_dormant_configmap_value() {
        let manifest = include_str!("../../../../infra/api/configmap.yaml");
        for (key, value) in [
            ("CHRONO_SPONSORSHIP_ENABLED", "false"),
            ("CHRONO_SPONSORSHIP_KILL_SWITCH_ENGAGED", "true"),
        ] {
            let declaration = format!("  {key}: \"{value}\"");
            assert!(
                manifest.lines().any(|line| line == declaration),
                "fixed ConfigMap is missing {declaration}"
            );
        }
        for key in SPONSORSHIP_REQUIRED_CONFIG_KEYS {
            let declaration = format!("  {key}: \"\"");
            assert!(
                manifest.lines().any(|line| line == declaration),
                "fixed ConfigMap is missing the dormant declaration {declaration}"
            );
        }
        for forbidden in [
            "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING",
            "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY",
        ] {
            assert!(
                !manifest
                    .lines()
                    .any(|line| line.trim_start().starts_with(&format!("{forbidden}:"))),
                "fixed ConfigMap must never carry inline secret key {forbidden}"
            );
        }
    }

    #[test]
    fn dormant_manifests_add_no_sponsorship_secret_or_projection_dependency() {
        let deployment = include_str!("../../../../infra/api/deployment.yaml");
        let provider_class = include_str!("../../../../infra/api/secretproviderclass.yaml");
        let secret = include_str!("../../../../infra/api/secret.yaml");

        let provider_class_bindings = deployment
            .lines()
            .filter(|line| line.trim_start().starts_with("secretProviderClass:"))
            .collect::<Vec<_>>();
        assert_eq!(
            provider_class_bindings,
            ["              secretProviderClass: cma-opensandbox-api-key"]
        );
        assert_eq!(deployment.matches("mountPath: /var/secrets").count(), 1);
        assert!(!deployment.contains("CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING_FILE"));
        assert!(!deployment.contains("CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY_FILE"));

        let resources = provider_class
            .lines()
            .filter(|line| line.trim_start().starts_with("- resourceName:"))
            .collect::<Vec<_>>();
        let paths = provider_class
            .lines()
            .filter(|line| line.trim_start().starts_with("path:"))
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 1);
        assert!(resources[0].contains("opensandbox-chrono-bot-api-key"));
        assert_eq!(paths, ["        path: \"opensandbox-api-key\""]);

        assert!(secret.lines().any(|line| line == "stringData: {}"));
        assert!(!secret.lines().any(|line| line.starts_with("data:")));
        for forbidden in [
            "CHRONO_SPONSORSHIP_ENCRYPTION_KEYRING",
            "CHRONO_SPONSORSHIP_AUDIT_HMAC_KEY",
        ] {
            assert!(
                !secret
                    .lines()
                    .any(|line| line.trim_start().starts_with(&format!("{forbidden}:"))),
                "Kubernetes Secret must not carry {forbidden}"
            );
        }
    }

    #[test]
    fn an_unselected_stage_observes_no_control_plane_configuration() {
        let mut accessed = Vec::new();
        let stage = MongoCutoverStageConfig::from_lookup(|key| {
            accessed.push(key.to_owned());
            Ok(None)
        })
        .expect("an absent stage is normal startup");
        assert!(stage.is_none());
        assert_eq!(accessed, [MONGODB_CUTOVER_STAGE]);
    }

    #[test]
    fn the_legacy_stage_reads_only_its_six_narrow_keys() {
        let values = BTreeMap::from([
            (MONGODB_CUTOVER_STAGE, "legacy"),
            ("CHRONO_MONGODB_LEGACY_CUTOVER", "drained"),
            ("CHRONO_MONGODB_COORDINATION_CUTOVER", ""),
            ("CHRONO_SERVER_BIND", "127.0.0.1:18080"),
            ("CHRONO_MONGODB_URI", "mongodb://stage.invalid/cma"),
            ("CHRONO_MONGODB_DATABASE", "stage_boundary"),
        ]);
        let mut accessed = Vec::new();
        let config = MongoCutoverStageConfig::from_lookup(|key| {
            accessed.push(key.to_owned());
            values
                .get(key)
                .map(|value| Some((*value).to_owned()))
                .ok_or_else(|| format!("stage read forbidden configuration key {key}"))
        })
        .expect("the narrow configuration is sufficient")
        .expect("stage is selected");
        assert_eq!(config.bind, "127.0.0.1:18080");
        assert_eq!(config.mongodb_database, "stage_boundary");
        assert_eq!(
            accessed,
            [
                MONGODB_CUTOVER_STAGE,
                "CHRONO_MONGODB_LEGACY_CUTOVER",
                "CHRONO_MONGODB_COORDINATION_CUTOVER",
                "CHRONO_SERVER_BIND",
                "CHRONO_MONGODB_URI",
                "CHRONO_MONGODB_DATABASE",
            ]
        );
    }

    #[test]
    fn the_stage_refuses_an_unreadable_environment_value() {
        let error = MongoCutoverStageConfig::from_lookup(|key| {
            Err(format!(
                "{key} is not valid UTF-8 and cannot be interpreted"
            ))
        })
        .expect_err("a non-text selector must not silently become normal startup");
        assert!(error.contains(MONGODB_CUTOVER_STAGE), "got {error}");
    }

    #[test]
    fn auth_mode_names_match_the_variants() {
        let c = Config::sample();
        assert_eq!(c.auth_mode_name(), "trusted");

        let c = Config {
            auth_mode: AuthMode::Provider {
                base_url: "http://p".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://x".into(),
            },
            ..c
        };
        assert_eq!(c.auth_mode_name(), "provider");
    }
}
