//! One platform spend period: model liability mirrored from owner rows, the
//! latest cloud observation and the typed closures that form the platform
//! admission state. Pure accounting; persistence and transactions live below.

use serde::{Deserialize, Serialize};

use super::{BudgetError, PlatformBudgetPolicy};

const SAFE_TOTAL: u64 = i64::MAX as u64;
const SECONDS_PER_DAY: u128 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureReason {
    /// committed ≥ warning level: new registrations are refused.
    SpendWarning,
    /// committed ≥ disposable: new commitments are refused and running work stops.
    SpendStop,
    /// A required source is stale, missing or inconsistent: new commitments are refused.
    MeteringUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Closure {
    pub reason: ClosureReason,
    pub since: u64,
    /// Settings revision under which the closure was entered.
    pub settings_revision: u64,
    /// Evidence only; never authority.
    pub committed_at_entry: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudObservation {
    pub observed_through: u64,
    pub units: u64,
    pub recorded_at: u64,
    pub settings_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPeriodLedger {
    pub period_start: u64,
    pub period_end: u64,
    /// Revision the levels were last computed under.
    pub settings_revision: u64,
    /// Valuation pinned on first liability; it cannot change under liability.
    pub token_units: Option<[u64; 2]>,
    pub spent: u64,
    pub reserved: u64,
    pub adjustments: u64,
    /// Latest observation; history lives in separate observation records.
    pub cloud: Option<CloudObservation>,
    pub closures: Vec<Closure>,
    pub evaluated_at: u64,
    /// Row compare-and-swap counter, incremented by every persisting writer.
    pub revision: u64,
}

/// A refused admission leaves the ledger untouched. `closures` are what the
/// caller enters outside its aborted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: BudgetError,
    pub closures: Vec<Closure>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evaluation {
    pub entered: Vec<Closure>,
    pub lifted: Vec<ClosureReason>,
}

impl Evaluation {
    pub fn changed(&self) -> bool {
        !self.entered.is_empty() || !self.lifted.is_empty()
    }
}

impl PlatformPeriodLedger {
    pub fn open(
        policy: &PlatformBudgetPolicy,
        settings_revision: u64,
    ) -> Result<Self, BudgetError> {
        let (period_start, period_end) = policy.period_bounds()?;
        Ok(Self {
            period_start,
            period_end,
            settings_revision,
            token_units: None,
            spent: 0,
            reserved: 0,
            adjustments: 0,
            cloud: None,
            closures: Vec::new(),
            evaluated_at: 0,
            revision: 0,
        })
    }

    /// Latest observed cloud units plus a rounded-up linear projection to `now`.
    /// An unconfigured, missing, future or stale source is unavailable, never zero.
    pub fn cloud_committed(
        &self,
        policy: &PlatformBudgetPolicy,
        now: u64,
    ) -> Result<u64, BudgetError> {
        let cloud = policy.cloud.as_ref().ok_or(BudgetError::Unavailable)?;
        let observed = self.cloud.as_ref().ok_or(BudgetError::Unavailable)?;
        let age = now
            .checked_sub(observed.observed_through)
            .filter(|age| *age <= cloud.max_observation_age_seconds)
            .ok_or(BudgetError::Unavailable)?;
        let projected =
            (u128::from(age) * u128::from(cloud.run_rate_units_per_day)).div_ceil(SECONDS_PER_DAY);
        u64::try_from(projected)
            .ok()
            .and_then(|projected| observed.units.checked_add(projected))
            .filter(|total| *total <= SAFE_TOTAL)
            .ok_or(BudgetError::Unavailable)
    }

    pub fn committed(&self, policy: &PlatformBudgetPolicy, now: u64) -> Result<u64, BudgetError> {
        let cloud = self.cloud_committed(policy, now)?;
        [self.reserved, cloud, self.adjustments]
            .into_iter()
            .try_fold(self.spent, |total, value| total.checked_add(value))
            .filter(|total| *total <= SAFE_TOTAL)
            .ok_or(BudgetError::Unavailable)
    }

    /// Admit `liability` (zero for commitments without a model component).
    /// Success mirrors the liability and enters `SpendWarning` in place;
    /// a refusal leaves `self` untouched.
    pub fn admit(
        &mut self,
        policy: &PlatformBudgetPolicy,
        settings_revision: u64,
        token_units: [u64; 2],
        liability: u64,
        now: u64,
    ) -> Result<Vec<Closure>, Refusal> {
        if self.has(ClosureReason::SpendStop) {
            return Err(Refusal {
                reason: BudgetError::PlatformExhausted,
                closures: Vec::new(),
            });
        }
        let mut next = self.clone();
        let committed = match next.admissible(policy, token_units, liability, now) {
            Ok(committed) => committed,
            Err(_) => return Err(self.unavailable(settings_revision, now, 0)),
        };
        if self.has(ClosureReason::MeteringUnavailable) {
            return Err(Refusal {
                reason: BudgetError::Unavailable,
                closures: Vec::new(),
            });
        }
        let levels = match policy.levels() {
            Ok(levels) => levels,
            Err(_) => return Err(self.unavailable(settings_revision, now, committed)),
        };
        let Some(total) = committed
            .checked_add(liability)
            .filter(|total| *total <= SAFE_TOTAL)
        else {
            return Err(self.unavailable(settings_revision, now, committed));
        };
        if total > levels.disposable {
            let closures = [ClosureReason::SpendStop, ClosureReason::SpendWarning]
                .into_iter()
                .filter(|reason| !self.has(*reason))
                .map(|reason| closure(reason, now, settings_revision, committed))
                .collect();
            return Err(Refusal {
                reason: BudgetError::PlatformExhausted,
                closures,
            });
        }
        next.reserved = next.reserved.saturating_add(liability);
        next.settings_revision = next.settings_revision.max(settings_revision);
        let mut entered = Vec::new();
        if total >= levels.warning && !next.has(ClosureReason::SpendWarning) {
            let warning = closure(ClosureReason::SpendWarning, now, settings_revision, total);
            next.closures.push(warning.clone());
            entered.push(warning);
        }
        *self = next;
        Ok(entered)
    }

    /// Apply the fail-closed conditions to `self` (a scratch copy) and return
    /// `committed(now)`.
    fn admissible(
        &mut self,
        policy: &PlatformBudgetPolicy,
        token_units: [u64; 2],
        liability: u64,
        now: u64,
    ) -> Result<u64, BudgetError> {
        self.adopt_period(policy)?;
        if now >= self.period_end {
            return Err(BudgetError::Unavailable);
        }
        if liability > 0 {
            if self.has_liability() && self.token_units != Some(token_units) {
                return Err(BudgetError::Unavailable);
            }
            self.token_units = Some(token_units);
        }
        self.committed(policy, now)
    }

    /// The row key is the period start; a different end is the same period and
    /// may change only while the row carries no liability.
    fn adopt_period(&mut self, policy: &PlatformBudgetPolicy) -> Result<(), BudgetError> {
        let (start, end) = policy.period_bounds()?;
        if start != self.period_start {
            return Err(BudgetError::Unavailable);
        }
        if end != self.period_end {
            if self.has_liability() {
                return Err(BudgetError::Unavailable);
            }
            self.period_end = end;
        }
        Ok(())
    }

    fn has_liability(&self) -> bool {
        self.spent != 0 || self.reserved != 0 || self.adjustments != 0
    }

    fn unavailable(&self, settings_revision: u64, now: u64, committed: u64) -> Refusal {
        let closures = if self.has(ClosureReason::MeteringUnavailable) {
            Vec::new()
        } else {
            vec![closure(
                ClosureReason::MeteringUnavailable,
                now,
                settings_revision,
                committed,
            )]
        };
        Refusal {
            reason: BudgetError::Unavailable,
            closures,
        }
    }

    pub fn settle(&mut self, reserved: u64, debit: u64) -> Result<(), BudgetError> {
        let remaining = self
            .reserved
            .checked_sub(reserved)
            .ok_or(BudgetError::Unavailable)?;
        let spent = self
            .spent
            .checked_add(debit)
            .filter(|spent| *spent <= SAFE_TOTAL)
            .ok_or(BudgetError::Unavailable)?;
        self.reserved = remaining;
        self.spent = spent;
        Ok(())
    }

    pub fn release(&mut self, reserved: u64) -> Result<(), BudgetError> {
        self.reserved = self
            .reserved
            .checked_sub(reserved)
            .ok_or(BudgetError::Unavailable)?;
        Ok(())
    }

    /// Late, larger usage is already incurred: no admission check applies.
    pub fn retain(&mut self, extra: u64) -> Result<(), BudgetError> {
        self.reserved = self
            .reserved
            .checked_add(extra)
            .filter(|reserved| *reserved <= SAFE_TOTAL)
            .ok_or(BudgetError::Unavailable)?;
        Ok(())
    }

    /// Re-evaluate time-driven transitions. Entering is monotonic; a spend
    /// closure lifts only under a strictly newer settings revision, while
    /// `MeteringUnavailable` lifts as soon as its cause clears.
    pub fn evaluate(
        &mut self,
        policy: &PlatformBudgetPolicy,
        settings_revision: u64,
        now: u64,
    ) -> Result<Evaluation, BudgetError> {
        let mut evaluation = Evaluation::default();
        let period = self.adopt_period(policy);
        let committed = period
            .and_then(|()| self.committed(policy, now))
            .and_then(|committed| policy.levels().map(|levels| (committed, levels)));
        let fault = committed.is_err() || now >= self.period_end;
        let evidence = committed.map(|(committed, _)| committed).unwrap_or(0);
        self.transition(
            ClosureReason::MeteringUnavailable,
            fault,
            !fault,
            (now, settings_revision, evidence),
            &mut evaluation,
        );
        if let Ok((committed, levels)) = committed {
            for (reason, level) in [
                (ClosureReason::SpendWarning, levels.warning),
                (ClosureReason::SpendStop, levels.disposable),
            ] {
                let newer = self.closures.iter().any(|closure| {
                    closure.reason == reason && settings_revision > closure.settings_revision
                });
                self.transition(
                    reason,
                    committed >= level,
                    newer && committed < level,
                    (now, settings_revision, committed),
                    &mut evaluation,
                );
            }
        }
        self.settings_revision = self.settings_revision.max(settings_revision);
        self.evaluated_at = now;
        Ok(evaluation)
    }

    fn transition(
        &mut self,
        reason: ClosureReason,
        enter: bool,
        lift: bool,
        (now, settings_revision, committed): (u64, u64, u64),
        evaluation: &mut Evaluation,
    ) {
        if self.has(reason) {
            let revision_allows = self.closures.iter().any(|closure| {
                closure.reason == reason && settings_revision >= closure.settings_revision
            });
            if lift && revision_allows {
                self.closures.retain(|closure| closure.reason != reason);
                evaluation.lifted.push(reason);
            }
        } else if enter {
            let entered = closure(reason, now, settings_revision, committed);
            self.closures.push(entered.clone());
            evaluation.entered.push(entered);
        }
    }

    /// A later observation supersedes an earlier one for the admission figure.
    pub fn record_cloud(&mut self, observation: CloudObservation) -> Result<(), BudgetError> {
        if observation.observed_through < self.period_start
            || observation.observed_through > self.period_end
            || observation.units > SAFE_TOTAL
        {
            return Err(BudgetError::InvalidPolicy);
        }
        if self
            .cloud
            .as_ref()
            .is_none_or(|current| observation.observed_through >= current.observed_through)
        {
            self.cloud = Some(observation);
        }
        Ok(())
    }

    pub fn closed_for_commitments(&self) -> bool {
        self.has(ClosureReason::SpendStop) || self.has(ClosureReason::MeteringUnavailable)
    }

    pub fn closed_for_registration(&self) -> bool {
        !self.closures.is_empty()
    }

    fn has(&self, reason: ClosureReason) -> bool {
        self.closures.iter().any(|closure| closure.reason == reason)
    }
}

fn closure(reason: ClosureReason, since: u64, settings_revision: u64, committed: u64) -> Closure {
    Closure {
        reason,
        since,
        settings_revision,
        committed_at_entry: committed,
    }
}
