use serde::{Deserialize, Serialize};

use super::BudgetError;

/// Explicit reporting interval and privately supplied platform stop policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformBudgetPolicy {
    pub period_cap: u64,
    pub period_start: String,
    pub period_end: String,
    pub warning_percent: u8,
    pub stop_percent: u8,
    pub protected_reserve: u64,
    pub lag_allowance: u64,
}

impl PlatformBudgetPolicy {
    pub fn validate(&self) -> Result<(), BudgetError> {
        let maximum = (1_u64 << 53) - 1;
        if self.period_cap == 0
            || self.period_cap > maximum
            || self.protected_reserve > maximum
            || self.lag_allowance > maximum
            || self.warning_percent == 0
            || self.warning_percent >= self.stop_percent
            || self.stop_percent > 100
        {
            return Err(BudgetError::InvalidPolicy);
        }
        let start =
            crate::seconds_from_rfc3339(&self.period_start).ok_or(BudgetError::InvalidPolicy)?;
        let end =
            crate::seconds_from_rfc3339(&self.period_end).ok_or(BudgetError::InvalidPolicy)?;
        let stop = u128::from(self.period_cap) * u128::from(self.stop_percent) / 100;
        let protected = u128::from(self.protected_reserve) + u128::from(self.lag_allowance);
        if start >= end || protected >= stop {
            return Err(BudgetError::InvalidPolicy);
        }
        Ok(())
    }
}
