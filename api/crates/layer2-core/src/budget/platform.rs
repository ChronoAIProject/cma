use serde::{Deserialize, Serialize};

use super::BudgetError;

const MAXIMUM: u64 = (1_u64 << 53) - 1;

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
    /// Absent means the cloud source is not configured: an enabled policy is
    /// then metering-unavailable, never a zero cloud figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudSpendPolicy>,
}

/// Operator-configured projection of the cloud bill between observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudSpendPolicy {
    pub run_rate_units_per_day: u64,
    pub max_observation_age_seconds: u64,
}

impl CloudSpendPolicy {
    pub fn validate(&self) -> Result<(), BudgetError> {
        if [
            self.run_rate_units_per_day,
            self.max_observation_age_seconds,
        ]
        .iter()
        .any(|value| *value == 0 || *value > MAXIMUM)
        {
            return Err(BudgetError::InvalidPolicy);
        }
        Ok(())
    }
}

/// Levels derived from one settings revision, in private accounting units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendLevels {
    pub warning: u64,
    pub stop: u64,
    pub disposable: u64,
}

impl PlatformBudgetPolicy {
    pub fn validate(&self) -> Result<(), BudgetError> {
        if self.period_cap == 0
            || self.period_cap > MAXIMUM
            || self.protected_reserve > MAXIMUM
            || self.lag_allowance > MAXIMUM
            || self.warning_percent == 0
            || self.warning_percent >= self.stop_percent
            || self.stop_percent > 100
        {
            return Err(BudgetError::InvalidPolicy);
        }
        if let Some(cloud) = &self.cloud {
            cloud.validate()?;
        }
        self.period_bounds()?;
        self.levels()?;
        Ok(())
    }

    /// `disposable = stop − protected_reserve − lag_allowance`, which must be positive.
    pub fn levels(&self) -> Result<SpendLevels, BudgetError> {
        let level = |percent: u8| {
            u64::try_from(u128::from(self.period_cap) * u128::from(percent) / 100)
                .map_err(|_| BudgetError::InvalidPolicy)
        };
        let stop = level(self.stop_percent)?;
        let disposable = self
            .protected_reserve
            .checked_add(self.lag_allowance)
            .and_then(|protected| stop.checked_sub(protected))
            .filter(|disposable| *disposable > 0)
            .ok_or(BudgetError::InvalidPolicy)?;
        Ok(SpendLevels {
            warning: level(self.warning_percent)?,
            stop,
            disposable,
        })
    }

    /// The period as `(start, end)` seconds; the row is keyed by its start.
    pub fn period_bounds(&self) -> Result<(u64, u64), BudgetError> {
        let start =
            crate::seconds_from_rfc3339(&self.period_start).ok_or(BudgetError::InvalidPolicy)?;
        let end =
            crate::seconds_from_rfc3339(&self.period_end).ok_or(BudgetError::InvalidPolicy)?;
        if start >= end {
            return Err(BudgetError::InvalidPolicy);
        }
        Ok((start, end))
    }
}
