use std::collections::BTreeMap;

use layer2_core::{PlatformSettingsService, StoredPlatformSettings};
use serde::Serialize;
use utoipa::ToSchema;

use super::body::PlatformSettingsBody;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SettingChangeView {
    actor_reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor_name: Option<String>,
    at: String,
    revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SettingsView {
    revision: u64,
    settings: PlatformSettingsBody,
    model_budget_writes_available: bool,
    model_request_budget_writes_available: bool,
    platform_spend_writes_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    setting_changes: Option<BTreeMap<String, SettingChangeView>>,
}

impl SettingsView {
    pub(super) fn new(
        value: StoredPlatformSettings,
        service: &PlatformSettingsService,
        allowed: bool,
        names: &BTreeMap<String, String>,
    ) -> Self {
        Self {
            revision: value.revision,
            settings: value.settings.into(),
            model_budget_writes_available: service.model_budget_writes_enabled(),
            model_request_budget_writes_available: service.model_request_budget_writes_enabled(),
            platform_spend_writes_available: service.platform_spend_writes_enabled(),
            setting_changes: allowed.then(|| {
                value
                    .changes
                    .into_iter()
                    .map(|(key, change)| {
                        (
                            key,
                            SettingChangeView {
                                actor_reference: super::super::activity::actor_reference(
                                    &change.actor.0,
                                ),
                                actor_name: names.get(&change.actor.0).cloned(),
                                at: change.at,
                                revision: change.revision,
                            },
                        )
                    })
                    .collect()
            }),
        }
    }
}
