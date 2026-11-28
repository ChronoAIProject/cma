//! Last change to each setting shown in Operations.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{budget::PlatformBudgetPolicy, Caller, PlatformSettings, SubjectId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSettingChange {
    pub actor: SubjectId,
    pub at: String,
    pub revision: u64,
}

pub type PlatformSettingChanges = BTreeMap<String, PlatformSettingChange>;

fn period(policy: &PlatformBudgetPolicy) -> (u64, &str, &str, u8, u8, u64, u64) {
    (
        policy.period_cap,
        &policy.period_start,
        &policy.period_end,
        policy.warning_percent,
        policy.stop_percent,
        policy.protected_reserve,
        policy.lag_allowance,
    )
}

pub(super) fn changes(
    current: &super::StoredPlatformSettings,
    next: &PlatformSettings,
    caller: &Caller,
) -> PlatformSettingChanges {
    let before = &current.settings;
    let spend = &before.model_budget;
    let next_spend = &next.model_budget;
    let changed = [
        ("spend_controls", spend.enabled != next_spend.enabled),
        (
            "platform_period",
            spend.platform.as_ref().map(period) != next_spend.platform.as_ref().map(period),
        ),
        (
            "cloud_metering",
            spend.platform.as_ref().and_then(|p| p.cloud.as_ref())
                != next_spend.platform.as_ref().and_then(|p| p.cloud.as_ref()),
        ),
        (
            "default_owner_policy",
            spend.default_policy != next_spend.default_policy,
        ),
        ("owner_overrides", spend.owners != next_spend.owners),
        (
            "request_models",
            spend.request_models != next_spend.request_models,
        ),
        (
            "sandbox_profile",
            before.defaults.sandbox_profile != next.defaults.sandbox_profile,
        ),
        (
            "workspace_profile",
            before.defaults.workspace_profile != next.defaults.workspace_profile,
        ),
        (
            "agent_profile",
            before.defaults.agent_profile != next.defaults.agent_profile,
        ),
        (
            "auto_pause",
            before.inactivity.auto_pause_enabled != next.inactivity.auto_pause_enabled,
        ),
        (
            "idle_threshold",
            before.inactivity.pause_after_seconds != next.inactivity.pause_after_seconds,
        ),
        (
            "measured_usage",
            before.usage_reporting != next.usage_reporting,
        ),
        (
            "agent_log_retention",
            before.retention.agent_log_days != next.retention.agent_log_days,
        ),
        (
            "sandboxes_per_user",
            before.limits.max_sandboxes_per_user != next.limits.max_sandboxes_per_user,
        ),
        (
            "visitor_sandboxes",
            before.limits.visitor_max_sandboxes != next.limits.visitor_max_sandboxes,
        ),
        (
            "visitor_launches",
            before.limits.visitor_max_launches_per_day != next.limits.visitor_max_launches_per_day,
        ),
        (
            "triggers_enabled",
            before.triggers.enabled != next.triggers.enabled,
        ),
        (
            "trigger_orchestration",
            before.triggers.orchestration_paused != next.triggers.orchestration_paused,
        ),
        (
            "marketplace",
            before.marketplace.enabled != next.marketplace.enabled,
        ),
    ];
    let stamp = PlatformSettingChange {
        actor: caller.subject.clone(),
        at: crate::now_rfc3339(),
        revision: current.revision.saturating_add(1),
    };
    let mut result = current.changes.clone();
    for (key, different) in changed {
        if different {
            result.insert(key.into(), stamp.clone());
        }
    }
    result
}
