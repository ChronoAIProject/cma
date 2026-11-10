use layer2_core::budget::{BudgetPolicy, BudgetSettings, OwnerBudgetPolicy};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_policy: Option<BudgetPolicyBody>,
    owners: Vec<OwnerBudgetPolicyBody>,
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
    input_token_units: u64,
    output_token_units: u64,
    max_unobserved_units: u64,
    usage_timeout_seconds: u64,
}

impl From<BudgetSettingsBody> for BudgetSettings {
    fn from(value: BudgetSettingsBody) -> Self {
        Self {
            default_policy: value.default_policy.map(Into::into),
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
            default_policy: value.default_policy.map(Into::into),
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
            input_token_units: value.input_token_units,
            output_token_units: value.output_token_units,
            max_unobserved_units: value.max_unobserved_units,
            usage_timeout_seconds: value.usage_timeout_seconds,
        }
    }
}
