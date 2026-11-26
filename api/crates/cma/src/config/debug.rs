//! Explicit redaction for process configuration diagnostics.
use super::Config;

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field(
                "agent_access_retained_origins",
                &self.agent_access_retained_origins,
            )
            .field("nyxid_public_url", &self.nyxid_public_url)
            .field("cors_origins", &self.cors_origins)
            .field("rate_limits", &self.rate_limits)
            .field("control_admission", &self.control_admission)
            .field("public_stream_limit", &self.public_stream_limit)
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
            .field("activity_async_enabled", &self.activity_async_enabled)
            .field(
                "activity_operational_read_sampling_enabled",
                &self.activity_operational_read_sampling_enabled,
            )
            .field("default_profiles", &self.default_profiles)
            .field(
                "agent_manager_sandbox_capacity",
                &self.agent_manager_sandbox_capacity,
            )
            .field("auth_mode", &self.auth_mode)
            .field("features", &self.features)
            .field("home_backup", &self.home_backup)
            .finish()
    }
}
