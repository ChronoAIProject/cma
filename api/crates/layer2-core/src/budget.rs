//! Admission accounting, independent of customer wallets and spend providers.

use serde::{Deserialize, Serialize};

use crate::{AgentId, SubjectId};

mod ledger;
pub use ledger::{OwnerBudget, TurnReservation};
mod request;
pub use request::{
    ModelRequest, RequestBudget, RequestHold, RequestModelPolicy, RequestScope, RequestUsage,
};
mod period;
#[cfg(test)]
mod period_tests;
pub use period::{
    Closure, ClosureReason, CloudObservation, Evaluation, PlatformPeriodLedger, Refusal,
};
mod platform;
#[cfg(test)]
mod request_tests;
mod spend;
pub use platform::{CloudSpendPolicy, PlatformBudgetPolicy, SpendLevels};
pub use spend::{
    CloudObservationInput, ObservationError, OperatorAdmissionState, PlatformAdmissionState,
    PlatformSpendLedger, PlatformSpendState,
};
mod spend_service;
pub use spend_service::PlatformSpendService;
#[cfg(test)]
mod controls_tests;

#[cfg(test)]
mod request_rollout_tests;
#[cfg(test)]
mod rollout_tests;
#[cfg(test)]
mod tests;

/// Exact accounting units. Their valuation is configured privately by an operator.
/// There is deliberately no built-in price, allowance or currency conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetPolicy {
    pub owner_day_limit: u64,
    pub turn_limit: u64,
    pub max_calls_per_turn: u64,
    pub day_timezone: String,
    pub input_token_units: u64,
    pub output_token_units: u64,
    pub max_unobserved_units: u64,
    pub usage_timeout_seconds: u64,
}

impl BudgetPolicy {
    pub fn validate(&self) -> Result<(), BudgetError> {
        let values = [
            self.owner_day_limit,
            self.turn_limit,
            self.input_token_units,
            self.output_token_units,
            self.max_unobserved_units,
        ];
        if values
            .iter()
            .any(|value| *value == 0 || *value > (1_u64 << 53) - 1)
            || self.turn_limit > self.owner_day_limit
            || self.max_unobserved_units >= self.turn_limit
            || !(1..=300).contains(&self.usage_timeout_seconds)
            || self.max_calls_per_turn == 0
            || self.max_calls_per_turn > u64::from(u32::MAX)
            || self.day_timezone.len() > 128
            || jiff::tz::TimeZone::get(&self.day_timezone).is_err()
        {
            return Err(BudgetError::InvalidPolicy);
        }
        Ok(())
    }

    pub fn day(&self, now: u64) -> Result<u64, BudgetError> {
        let date = self.zoned(now)?.date();
        let month =
            time::Month::try_from(date.month() as u8).map_err(|_| BudgetError::Unavailable)?;
        let date = time::Date::from_calendar_date(i32::from(date.year()), month, date.day() as u8)
            .map_err(|_| BudgetError::Unavailable)?;
        let epoch = time::Date::from_calendar_date(1970, time::Month::January, 1)
            .map_err(|_| BudgetError::Unavailable)?;
        u64::try_from(date.to_julian_day() - epoch.to_julian_day())
            .map_err(|_| BudgetError::Unavailable)
    }

    /// The start of the next accounting day, as seconds since the Unix epoch.
    pub fn next_reset_at(&self, now: u64) -> Result<u64, BudgetError> {
        let current = self.zoned(now)?;
        let zone = current.time_zone();
        let tomorrow = current
            .date()
            .tomorrow()
            .map_err(|_| BudgetError::Unavailable)?;
        let midnight = zone.to_ambiguous_timestamp(tomorrow.at(0, 0, 0, 0));
        let reset = if matches!(midnight.offset(), jiff::tz::AmbiguousOffset::Gap { .. }) {
            // The clock can jump across midnight. The day starts at the transition.
            let before = midnight.earlier().map_err(|_| BudgetError::Unavailable)?;
            zone.following(before)
                .next()
                .ok_or(BudgetError::Unavailable)?
                .timestamp()
        } else {
            midnight.earlier().map_err(|_| BudgetError::Unavailable)?
        };
        u64::try_from(reset.as_second()).map_err(|_| BudgetError::Unavailable)
    }

    fn zoned(&self, now: u64) -> Result<jiff::Zoned, BudgetError> {
        let zone =
            jiff::tz::TimeZone::get(&self.day_timezone).map_err(|_| BudgetError::Unavailable)?;
        Ok(
            jiff::Timestamp::from_second(i64::try_from(now).map_err(|_| BudgetError::Unavailable)?)
                .map_err(|_| BudgetError::Unavailable)?
                .to_zoned(zone),
        )
    }

    pub fn value_usage(&self, input: u64, output: u64) -> Result<u64, BudgetError> {
        input
            .checked_mul(self.input_token_units)
            .and_then(|input| {
                output
                    .checked_mul(self.output_token_units)
                    .and_then(|output| input.checked_add(output))
            })
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(BudgetError::Unavailable)
    }

    pub fn require_turn_headroom(&self, observed: u64) -> Result<(), BudgetError> {
        match observed.checked_add(self.max_unobserved_units) {
            Some(liability) if liability < self.turn_limit => Ok(()),
            _ => Err(BudgetError::TurnExhausted),
        }
    }
}

/// Explicit owner scope supports isolated production verification. An absent
/// policy leaves the existing admission contract in force during rollout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformBudgetPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_policy: Option<BudgetPolicy>,
    pub owners: Vec<OwnerBudgetPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_models: Vec<RequestModelPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerBudgetPolicy {
    pub owner: SubjectId,
    pub policy: BudgetPolicy,
}

impl BudgetSettings {
    /// Only this empty shape can be omitted for strict pre-budget readers.
    /// Disabled policies with retained values still require compatible readers.
    pub fn is_empty_disabled(&self) -> bool {
        !self.enabled
            && self.platform.is_none()
            && self.default_policy.is_none()
            && self.owners.is_empty()
            && self.request_models.is_empty()
    }

    pub fn validate(&self) -> Result<(), BudgetError> {
        if self.request_models.len() > 64 {
            return Err(BudgetError::InvalidPolicy);
        }
        let mut models = std::collections::HashSet::new();
        for model in &self.request_models {
            model.validate()?;
            if !models.insert(&model.model) {
                return Err(BudgetError::InvalidPolicy);
            }
        }
        if let Some(platform) = &self.platform {
            platform.validate()?;
        }
        if self.enabled
            && (self.platform.is_none()
                || (self.default_policy.is_none() && self.owners.is_empty()))
        {
            return Err(BudgetError::InvalidPolicy);
        }
        if let Some(policy) = &self.default_policy {
            policy.validate()?;
        }
        if self.owners.len() > 1024 {
            return Err(BudgetError::InvalidPolicy);
        }
        let mut owners = std::collections::HashSet::new();
        for entry in &self.owners {
            if entry.owner.as_str().trim().is_empty() || !owners.insert(&entry.owner) {
                return Err(BudgetError::InvalidPolicy);
            }
            entry.policy.validate()?;
        }
        Ok(())
    }

    pub fn for_owner(&self, owner: &SubjectId) -> Option<&BudgetPolicy> {
        if !self.enabled {
            return None;
        }
        self.owners
            .iter()
            .find(|entry| &entry.owner == owner)
            .map(|entry| &entry.policy)
            .or(self.default_policy.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum BudgetError {
    #[error("owner budget is exhausted")]
    OwnerExhausted,
    #[error("turn budget is exhausted")]
    TurnExhausted,
    #[error("budget usage is unavailable")]
    Unavailable,
    #[error("budget policy is invalid")]
    InvalidPolicy,
    #[error("platform spend limit is reached")]
    PlatformExhausted,
}

impl BudgetError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::OwnerExhausted => "owner_budget_exhausted",
            Self::TurnExhausted => "turn_budget_exhausted",
            Self::Unavailable | Self::InvalidPolicy => "budget_unavailable",
            Self::PlatformExhausted => "platform_budget_exhausted",
        }
    }

    pub const fn guidance(self) -> &'static str {
        match self {
            Self::OwnerExhausted => "HTTP 402: The owner's model budget is exhausted. After the budget is restored or its daily window resets, send a new message to continue. Completed work is preserved; this turn will not replay automatically.",
            Self::TurnExhausted => "HTTP 402: This turn reached its model budget. Send a new message with a smaller scope after budget is available. Completed work is preserved; this turn will not replay automatically.",
            Self::Unavailable | Self::InvalidPolicy => "HTTP 402: Model budget accounting is unavailable. After accounting is restored, send a new message to continue. Completed work is preserved; this turn will not replay automatically.",
            Self::PlatformExhausted => "The platform's spend limit for this period does not allow new work right now. Completed work is preserved. After the limit is raised, send a new message to continue; this turn will not replay automatically.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetStop {
    pub agent_id: AgentId,
    pub runtime_generation: u64,
    pub turn_sequence: u64,
    pub reason: BudgetError,
}

/// A replaceable persisted-usage boundary. Implementations must derive owner
/// scope from the stored Agent, never from an untrusted caller-supplied owner.
#[async_trait::async_trait]
pub trait SpendSource: Send + Sync {
    async fn stop_requested(
        &self,
        agent_id: &AgentId,
        runtime_generation: u64,
        now_seconds: u64,
    ) -> Result<Option<BudgetStop>, BudgetError>;
}
