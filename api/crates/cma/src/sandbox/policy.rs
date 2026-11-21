use std::sync::Arc;
use std::time::Duration;

use layer2_core::sandbox::{SandboxPolicy, SandboxPolicySource};
use layer2_core::{CoreError, PlatformSettingsService, ServiceId, StoreError};

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Translate every live platform-settings read into the sandbox policy shape.
pub(crate) struct LiveSandboxPolicy {
    settings: Arc<PlatformSettingsService>,
    baseline_services: Vec<ServiceId>,
}

impl LiveSandboxPolicy {
    pub(crate) fn new(
        settings: Arc<PlatformSettingsService>,
        baseline_services: Vec<ServiceId>,
    ) -> Self {
        Self {
            settings,
            baseline_services,
        }
    }
}

#[async_trait::async_trait]
impl SandboxPolicySource for LiveSandboxPolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, StoreError> {
        let stored = self.settings.current().await.map_err(settings_error)?;
        let max_sandboxes_per_owner = u16::try_from(stored.settings.limits.max_sandboxes_per_user)
            .map_err(|_| out_of_range("limits.max_sandboxes_per_user"))?;
        let max_sandboxes_per_visitor = u16::try_from(stored.settings.limits.visitor_max_sandboxes)
            .map_err(|_| out_of_range("limits.visitor_max_sandboxes"))?;
        let retention_seconds = u64::from(stored.settings.retention.checkpoint_grace_days)
            .checked_mul(SECONDS_PER_DAY)
            .ok_or_else(|| out_of_range("retention.checkpoint_grace_days"))?;

        Ok(SandboxPolicy {
            max_sandboxes_per_owner,
            max_sandboxes_per_visitor,
            inactivity_pause_after_seconds: stored.settings.inactivity.pause_after_seconds,
            checkpoint_interval_seconds: stored.settings.checkpoints.interval_seconds,
            retention_grace: Duration::from_secs(retention_seconds),
            baseline_services: self.baseline_services.clone(),
        })
    }

    async fn automatic_pause_allowed(&self, now: &str) -> Result<bool, StoreError> {
        let stored = self.settings.current().await.map_err(settings_error)?;
        if !stored.settings.inactivity.auto_pause_enabled {
            return Ok(false);
        }
        let Some(started) = stored.settings.inactivity.grace_started_at.as_deref() else {
            return Ok(false);
        };
        let Some(started) = layer2_core::seconds_from_rfc3339(started) else {
            return Ok(false);
        };
        let Some(now) = layer2_core::seconds_from_rfc3339(now) else {
            return Ok(false);
        };
        Ok(now >= started.saturating_add(stored.settings.inactivity.pause_after_seconds))
    }
}

fn settings_error(error: CoreError) -> StoreError {
    match error {
        CoreError::RuntimeUnavailable(detail) => StoreError::Unavailable(detail),
        other => StoreError::Malformed(other.to_string()),
    }
}

fn out_of_range(field: &str) -> StoreError {
    StoreError::Malformed(format!(
        "platform setting {field} exceeds the sandbox policy representation"
    ))
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
