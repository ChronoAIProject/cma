use std::sync::Arc;

use layer2_core::trigger_domain::{LaunchPolicy, LaunchPolicySource, LaunchServiceError};
use layer2_core::PlatformSettingsService;

const SETTINGS_UNAVAILABLE: &str = "platform settings are unavailable";

/// Translate every live platform-settings read into the Trigger launch policy.
pub(crate) struct LiveLaunchPolicy {
    settings: Arc<PlatformSettingsService>,
}

impl LiveLaunchPolicy {
    pub(crate) fn new(settings: Arc<PlatformSettingsService>) -> Self {
        Self { settings }
    }
}

#[async_trait::async_trait]
impl LaunchPolicySource for LiveLaunchPolicy {
    async fn current_launch_policy(&self) -> Result<LaunchPolicy, LaunchServiceError> {
        let stored = self.settings.current().await.map_err(|error| {
            tracing::error!(error = %error, "could not read Trigger launch policy");
            LaunchServiceError::Unavailable(SETTINGS_UNAVAILABLE.into())
        })?;

        Ok(LaunchPolicy {
            enabled: stored.settings.triggers.enabled,
            orchestration_paused: stored.settings.triggers.orchestration_paused,
            visitor_launches_per_day: stored.settings.limits.visitor_max_launches_per_day,
        })
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
