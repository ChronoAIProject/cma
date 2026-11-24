use crate::config::Config;
use layer2_core::{
    sandbox_usage_cursor::{UsageCursorPolicy, UsageCursors},
    sandbox_usage_service::{SandboxUsageService, SandboxUsageStore},
};
use std::sync::Arc;

pub(super) fn service(
    store: Arc<dyn SandboxUsageStore>,
    config: &Config,
) -> Arc<SandboxUsageService> {
    let policy = if config.features.usage_strict_cursors_enabled {
        UsageCursorPolicy::Strict
    } else if config.features.usage_signed_cursors_enabled {
        UsageCursorPolicy::IssueSigned
    } else {
        UsageCursorPolicy::ReaderOnly
    };
    let key = config
        .auth_encryption_key
        .usage_cursor_key(&config.deployment_id);
    let cursors = UsageCursors::new(&key, policy)
        .with_snapshot_issuance(config.features.usage_snapshot_cursors_enabled);
    SandboxUsageService::new(store, cursors)
}
