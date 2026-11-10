use layer2_core::budget::{
    BudgetPolicy, BudgetSettings, OwnerBudgetPolicy, PlatformBudgetPolicy, RequestModelPolicy,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub(super) fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BudgetSettingsBody>, D::Error> {
    BudgetSettingsBody::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BudgetSettingsBody {
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    platform: Option<PlatformBudgetPolicyBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_policy: Option<BudgetPolicyBody>,
    owners: Vec<OwnerBudgetPolicyBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    request_models: Vec<RequestModelPolicyBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RequestModelPolicyBody {
    model: String,
    input_overhead_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PlatformBudgetPolicyBody {
    period_cap: u64,
    period_start: String,
    period_end: String,
    warning_percent: u8,
    stop_percent: u8,
    protected_reserve: u64,
    lag_allowance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct OwnerBudgetPolicyBody {
    owner: String,
    policy: BudgetPolicyBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct BudgetPolicyBody {
    owner_day_limit: u64,
    turn_limit: u64,
    max_calls_per_turn: u64,
    day_timezone: String,
    input_token_units: u64,
    output_token_units: u64,
    max_unobserved_units: u64,
    usage_timeout_seconds: u64,
}

impl From<BudgetSettingsBody> for BudgetSettings {
    fn from(value: BudgetSettingsBody) -> Self {
        Self {
            enabled: value.enabled,
            platform: value.platform.map(Into::into),
            default_policy: value.default_policy.map(Into::into),
            request_models: value
                .request_models
                .into_iter()
                .map(|model| RequestModelPolicy {
                    model: model.model,
                    input_overhead_tokens: model.input_overhead_tokens,
                })
                .collect(),
            owners: value
                .owners
                .into_iter()
                .map(|entry| OwnerBudgetPolicy {
                    owner: layer2_core::SubjectId(entry.owner),
                    policy: entry.policy.into(),
                })
                .collect(),
        }
    }
}

impl From<BudgetSettings> for BudgetSettingsBody {
    fn from(value: BudgetSettings) -> Self {
        Self {
            enabled: value.enabled,
            platform: value.platform.map(Into::into),
            default_policy: value.default_policy.map(Into::into),
            request_models: value
                .request_models
                .into_iter()
                .map(|model| RequestModelPolicyBody {
                    model: model.model,
                    input_overhead_tokens: model.input_overhead_tokens,
                })
                .collect(),
            owners: value
                .owners
                .into_iter()
                .map(|entry| OwnerBudgetPolicyBody {
                    owner: entry.owner.0,
                    policy: entry.policy.into(),
                })
                .collect(),
        }
    }
}

impl From<BudgetPolicyBody> for BudgetPolicy {
    fn from(value: BudgetPolicyBody) -> Self {
        Self {
            owner_day_limit: value.owner_day_limit,
            turn_limit: value.turn_limit,
            max_calls_per_turn: value.max_calls_per_turn,
            day_timezone: value.day_timezone,
            input_token_units: value.input_token_units,
            output_token_units: value.output_token_units,
            max_unobserved_units: value.max_unobserved_units,
            usage_timeout_seconds: value.usage_timeout_seconds,
        }
    }
}
impl From<BudgetPolicy> for BudgetPolicyBody {
    fn from(value: BudgetPolicy) -> Self {
        Self {
            owner_day_limit: value.owner_day_limit,
            turn_limit: value.turn_limit,
            max_calls_per_turn: value.max_calls_per_turn,
            day_timezone: value.day_timezone,
            input_token_units: value.input_token_units,
            output_token_units: value.output_token_units,
            max_unobserved_units: value.max_unobserved_units,
            usage_timeout_seconds: value.usage_timeout_seconds,
        }
    }
}

impl From<PlatformBudgetPolicyBody> for PlatformBudgetPolicy {
    fn from(value: PlatformBudgetPolicyBody) -> Self {
        Self {
            period_cap: value.period_cap,
            period_start: value.period_start,
            period_end: value.period_end,
            warning_percent: value.warning_percent,
            stop_percent: value.stop_percent,
            protected_reserve: value.protected_reserve,
            lag_allowance: value.lag_allowance,
        }
    }
}
impl From<PlatformBudgetPolicy> for PlatformBudgetPolicyBody {
    fn from(value: PlatformBudgetPolicy) -> Self {
        Self {
            period_cap: value.period_cap,
            period_start: value.period_start,
            period_end: value.period_end,
            warning_percent: value.warning_percent,
            stop_percent: value.stop_percent,
            protected_reserve: value.protected_reserve,
            lag_allowance: value.lag_allowance,
        }
    }
}
