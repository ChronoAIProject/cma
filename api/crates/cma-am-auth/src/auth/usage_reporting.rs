//! Read only the presentation policy for authenticated browser and bearer sessions.

use std::sync::Arc;

use layer2_core::{platform::UsageReportingSettings, CoreError, PlatformSettingsService};

use super::AuthState;

impl AuthState {
    pub fn with_platform_settings(
        mut self: Arc<Self>,
        settings: Arc<PlatformSettingsService>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure platform settings before sharing auth state")
            .platform_settings = Some(settings);
        self
    }

    pub(super) async fn usage_reporting(&self) -> Result<UsageReportingSettings, CoreError> {
        match &self.platform_settings {
            Some(service) => Ok(service.current().await?.settings.usage_reporting),
            None => Ok(UsageReportingSettings::default()),
        }
    }
}

#[cfg(test)]
#[path = "usage_reporting_tests.rs"]
mod tests;
