use serde::{Deserialize, Serialize};

use super::{BudgetError, BudgetPolicy};

/// Persisted with a real shared write in the same transaction as admission.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerBudget {
    pub day: u64,
    pub spent: u64,
    pub reserved: u64,
    pub token_units: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_timezone: Option<String>,
}

impl OwnerBudget {
    pub fn advance(&mut self, policy: &BudgetPolicy, now_seconds: u64) -> Result<(), BudgetError> {
        let day = policy.day(now_seconds)?;
        if self.day_timezone.as_deref() != Some(&policy.day_timezone) {
            if self.spent != 0 || self.reserved != 0 {
                return Err(BudgetError::Unavailable);
            }
            self.day = day;
        }
        if day < self.day {
            return Err(BudgetError::Unavailable);
        }
        if day > self.day {
            self.day = day;
            self.spent = 0;
            // An unresolved turn never becomes free merely because a day ended.
        }
        self.day_timezone = Some(policy.day_timezone.clone());
        Ok(())
    }

    pub fn reserve(&mut self, policy: &BudgetPolicy, now: u64) -> Result<(), BudgetError> {
        self.reserve_liability(policy, policy.turn_limit, now)
    }

    pub fn reserve_liability(
        &mut self,
        policy: &BudgetPolicy,
        liability: u64,
        now: u64,
    ) -> Result<(), BudgetError> {
        policy.validate()?;
        if liability == 0 {
            return Err(BudgetError::Unavailable);
        }
        let mut next = self.clone();
        next.advance(policy, now)?;
        let units = [policy.input_token_units, policy.output_token_units];
        if (next.spent != 0 || next.reserved != 0) && next.token_units != Some(units) {
            return Err(BudgetError::Unavailable);
        }
        next.token_units = Some(units);
        let reserved = next
            .reserved
            .checked_add(liability)
            .ok_or(BudgetError::Unavailable)?;
        if next
            .spent
            .checked_add(reserved)
            .is_none_or(|total| total > policy.owner_day_limit)
        {
            return Err(BudgetError::OwnerExhausted);
        }
        next.reserved = reserved;
        *self = next;
        Ok(())
    }

    pub fn settle(&mut self, reservation: &TurnReservation, now: u64) -> Result<(), BudgetError> {
        let mut next = self.clone();
        next.advance(&reservation.policy, now)?;
        next.reserved = next
            .reserved
            .checked_sub(reservation.reserved_units)
            .ok_or(BudgetError::Unavailable)?;
        // Charge cross-day settlement to the current window conservatively.
        // Missing final usage retains the full allowance, never an assumed zero.
        let debit = if reservation.final_usage_known {
            reservation.observed
        } else {
            reservation.observed.max(reservation.reserved_units)
        };
        next.spent = next
            .spent
            .checked_add(debit)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(BudgetError::Unavailable)?;
        *self = next;
        Ok(())
    }

    pub fn retain_observed_liability(
        &mut self,
        reservation: &mut TurnReservation,
    ) -> Result<(), BudgetError> {
        let required = reservation
            .observed
            .checked_add(reservation.policy.max_unobserved_units)
            .ok_or(BudgetError::Unavailable)?
            .max(reservation.reserved_units);
        let extra = required - reservation.reserved_units;
        let reserved = self
            .reserved
            .checked_add(extra)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(BudgetError::Unavailable)?;
        self.reserved = reserved;
        reservation.reserved_units = required;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnReservation {
    pub policy: BudgetPolicy,
    pub reserved_units: u64,
    pub observed: u64,
    pub last_usage_at: u64,
    pub final_usage_known: bool,
    pub stop: Option<BudgetError>,
    pub settled: bool,
}

impl TurnReservation {
    pub fn new(policy: BudgetPolicy, now: u64) -> Self {
        Self {
            reserved_units: policy.turn_limit,
            policy,
            observed: 0,
            last_usage_at: now,
            final_usage_known: false,
            stop: None,
            settled: false,
        }
    }

    pub fn observe(&mut self, input: u64, output: u64, now: u64) -> Result<(), BudgetError> {
        let units = self.policy.value_usage(input, output)?;
        if units < self.observed || now < self.last_usage_at || self.settled {
            return Err(BudgetError::Unavailable);
        }
        self.observed = units;
        self.last_usage_at = now;
        if let Err(reason) = self.policy.require_turn_headroom(units) {
            self.stop.get_or_insert(reason);
        }
        Ok(())
    }

    /// A lower live limit takes effect without releasing outstanding liability.
    /// Changing valuation cannot reinterpret reservations already made.
    pub fn revalidate(&mut self, policy: &BudgetPolicy) -> Result<(), BudgetError> {
        policy.validate()?;
        if self.policy.input_token_units != policy.input_token_units
            || self.policy.output_token_units != policy.output_token_units
            || self.policy.day_timezone != policy.day_timezone
        {
            return Err(BudgetError::Unavailable);
        }
        self.policy.turn_limit = self.policy.turn_limit.min(policy.turn_limit);
        self.policy.max_calls_per_turn = self
            .policy
            .max_calls_per_turn
            .min(policy.max_calls_per_turn);
        self.policy.max_unobserved_units = self
            .policy
            .max_unobserved_units
            .max(policy.max_unobserved_units);
        self.policy.usage_timeout_seconds = self
            .policy
            .usage_timeout_seconds
            .min(policy.usage_timeout_seconds);
        self.policy.require_turn_headroom(self.observed)
    }

    pub fn stop_reason(&self, now: u64) -> Option<BudgetError> {
        self.stop.or_else(|| {
            now.checked_sub(self.last_usage_at)
                .filter(|age| *age <= self.policy.usage_timeout_seconds)
                .is_none()
                .then_some(BudgetError::Unavailable)
        })
    }
}
