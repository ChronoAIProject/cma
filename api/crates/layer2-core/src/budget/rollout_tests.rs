//! Retained PlatformSettings shape from develop 15deaed86, before model_budget.
//! Unchanged nested settings types are reused; the predecessor root remains strict.
use crate::platform::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreBudgetSettings {
    #[serde(default, skip_serializing_if = "usage_disabled")]
    usage_reporting: UsageReportingSettings,
    defaults: PlatformDefaults,
    inactivity: InactivitySettings,
    #[serde(default)]
    checkpoints: CheckpointSettings,
    retention: RetentionSettings,
    limits: PlatformLimits,
    triggers: TriggerSettings,
    marketplace: MarketplaceSettings,
}

fn usage_disabled(value: &UsageReportingSettings) -> bool {
    !value.measured_visible
}

#[test]
fn empty_disabled_budget_is_omitted_and_readable_by_the_retained_predecessor() {
    let mut settings = PlatformSettings::default();
    settings.marketplace.enabled = false;
    let persisted = serde_json::to_value(&settings).unwrap();
    assert!(persisted.get("model_budget").is_none());
    let predecessor: PreBudgetSettings = serde_json::from_value(persisted.clone()).unwrap();
    assert_eq!(serde_json::to_value(predecessor).unwrap(), persisted);
    assert_eq!(
        serde_json::from_value::<PlatformSettings>(persisted).unwrap(),
        settings
    );
}

#[test]
fn retaining_disabled_values_requires_the_new_reader_before_the_first_save() {
    let mut settings = PlatformSettings::default();
    settings.model_budget.platform = Some(super::PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
    });
    settings.validate().unwrap();
    assert!(!settings.model_budget.enabled);
    assert!(!settings.model_budget.is_empty_disabled());
    let persisted = serde_json::to_value(&settings).unwrap();
    assert!(persisted.get("model_budget").is_some());
    assert!(serde_json::from_value::<PreBudgetSettings>(persisted.clone()).is_err());
    assert_eq!(
        serde_json::from_value::<PlatformSettings>(persisted).unwrap(),
        settings
    );
}
