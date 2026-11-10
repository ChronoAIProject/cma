//! Admission accounting, independent of customer wallets and spend providers.

use serde::{Deserialize, Serialize};

use crate::{AgentId, SubjectId};

mod ledger;
pub use ledger::{OwnerBudget, TurnReservation};
mod request;
pub use request::{
    ModelRequest, RequestBudget, RequestHold, RequestModelPolicy, RequestScope, RequestUsage,
};
#[cfg(test)]
mod request_tests;
#[cfg(test)]
mod tests;

/// Exact accounting units. Their valuation is configured privately by an operator.
/// There is deliberately no built-in price, allowance or currency conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetPolicy {
    pub owner_day_limit: u64,
    pub turn_limit: u64,
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
        {
            return Err(BudgetError::InvalidPolicy);
        }
        Ok(())
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
    pub fn is_disabled(&self) -> bool {
        self.default_policy.is_none() && self.owners.is_empty() && self.request_models.is_empty()
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
}

impl BudgetError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::OwnerExhausted => "owner_budget_exhausted",
            Self::TurnExhausted => "turn_budget_exhausted",
            Self::Unavailable | Self::InvalidPolicy => "budget_unavailable",
        }
    }

    pub const fn guidance(self) -> &'static str {
        match self {
            Self::OwnerExhausted => "HTTP 402: The owner's model budget is exhausted. After the budget is restored or its daily window resets, send a new message to continue. Completed work is preserved; this turn will not replay automatically.",
            Self::TurnExhausted => "HTTP 402: This turn reached its model budget. Send a new message with a smaller scope after budget is available. Completed work is preserved; this turn will not replay automatically.",
            Self::Unavailable | Self::InvalidPolicy => "HTTP 402: Model budget accounting is unavailable. After accounting is restored, send a new message to continue. Completed work is preserved; this turn will not replay automatically.",
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
