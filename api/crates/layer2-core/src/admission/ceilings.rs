//! Independently revised operator capacity policy and immutable evidence.

use crate::{StoreError, SubjectId};
use serde::{Deserialize, Serialize};

mod memory;
mod service;
pub use memory::MemoryAdmissionCeilingsStore;
pub use service::AdmissionCeilingsService;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapacityKind {
    #[error("registration capacity is unavailable")]
    Registrations,
    #[error("retained home capacity is unavailable")]
    RetainedHomes,
    #[error("active session capacity is unavailable")]
    ActiveSessions,
}

impl CapacityKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Registrations => "registration_closed",
            Self::RetainedHomes => "sandbox_capacity_unavailable",
            Self::ActiveSessions => "session_capacity_unavailable",
        }
    }
    pub const fn guidance(self) -> &'static str {
        match self {
            Self::Registrations => {
                "New sign-ups are temporarily closed. Existing accounts are unaffected."
            }
            Self::RetainedHomes => "Sandbox capacity is temporarily unavailable. Retry later.",
            Self::ActiveSessions => {
                "Active session capacity is temporarily unavailable. Retry later."
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExistingReserve {
    pub retained_homes: Option<u32>,
    pub active_sessions: Option<u32>,
}

/// None is unlimited. A configured zero admits no additional capacity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdmissionCeilings {
    pub registrations: Option<u32>,
    pub retained_homes: Option<u32>,
    pub active_sessions: Option<u32>,
    pub owner_active_sessions: Option<u32>,
    pub existing_reserve: ExistingReserve,
}

impl AdmissionCeilings {
    pub fn validate(&self) -> Result<(), StoreError> {
        for (reserve, ceiling) in [
            (self.existing_reserve.retained_homes, self.retained_homes),
            (self.existing_reserve.active_sessions, self.active_sessions),
        ] {
            if matches!((reserve, ceiling), (Some(reserve), Some(ceiling)) if reserve > ceiling) {
                return Err(StoreError::Malformed(
                    "existing reserve exceeds its ceiling".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn effective_retained_homes(&self, deployment_limit: u16) -> Option<u32> {
        match (self.retained_homes, deployment_limit) {
            (limit, 0) => limit,
            (Some(limit), hard_cap) => Some(limit.min(u32::from(hard_cap))),
            (None, hard_cap) => Some(u32::from(hard_cap)),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionCeilingsState {
    pub revision: u64,
    pub ceilings: AdmissionCeilings,
    pub last_change: Option<AdmissionCeilingsChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionCeilingsChange {
    pub revision: u64,
    pub at: u64,
    pub actor: SubjectId,
    pub reason: String,
    pub old_ceilings: AdmissionCeilings,
    pub new_ceilings: AdmissionCeilings,
}

impl AdmissionCeilingsState {
    pub fn validate(&self) -> Result<(), StoreError> {
        self.ceilings.validate()?;
        match &self.last_change {
            None if self.revision == 0 && self.ceilings == AdmissionCeilings::default() => Ok(()),
            Some(change)
                if change.revision == self.revision && change.new_ceilings == self.ceilings =>
            {
                change.validate()
            }
            _ => Err(StoreError::Malformed(
                "admission ceilings evidence is inconsistent".into(),
            )),
        }
    }
    pub fn changed(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: AdmissionCeilings,
        now: u64,
    ) -> Result<Option<Self>, StoreError> {
        self.validate()?;
        ceilings.validate()?;
        if self.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if !super::valid_reason(reason) || actor.as_str().is_empty() || now > i64::MAX as u64 {
            return Err(StoreError::Malformed(
                "admission ceilings change is invalid".into(),
            ));
        }
        if self.ceilings == ceilings {
            return Ok(None);
        }
        let revision = self
            .revision
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| StoreError::Malformed("admission ceilings revision overflow".into()))?;
        Ok(Some(Self {
            revision,
            ceilings: ceilings.clone(),
            last_change: Some(AdmissionCeilingsChange {
                revision,
                at: now,
                actor: actor.clone(),
                reason: reason.into(),
                old_ceilings: self.ceilings.clone(),
                new_ceilings: ceilings,
            }),
        }))
    }
}

impl AdmissionCeilingsChange {
    pub fn validate(&self) -> Result<(), StoreError> {
        self.old_ceilings.validate()?;
        self.new_ceilings.validate()?;
        if self.revision == 0
            || self.revision > i64::MAX as u64
            || self.at > i64::MAX as u64
            || self.actor.as_str().is_empty()
            || !super::valid_reason(&self.reason)
            || self.old_ceilings == self.new_ceilings
        {
            return Err(StoreError::Malformed(
                "admission ceilings change is inconsistent".into(),
            ));
        }
        Ok(())
    }
    pub fn activity_record(&self) -> Result<crate::ActivityRecord, StoreError> {
        self.validate()?;
        let instant = std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(self.at))
            .ok_or_else(|| StoreError::Malformed("admission ceilings timestamp overflow".into()))?;
        let mut record = crate::ActivityRecord::new(
            crate::ActivityAction::Configure,
            crate::ActivityOutcome::Succeeded,
        )
        .by(self.actor.clone())
        .on(crate::ActivityTarget::Resource("admission_ceilings".into()));
        record.at = crate::rfc3339_at(instant);
        record
            .detail
            .record(crate::DetailKey::Revision, self.revision.to_string());
        record.detail.record(crate::DetailKey::Status, "configured");
        Ok(record)
    }
}

/// A bounded observation is a lower bound when the count reaches its bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityCount {
    pub observed: u64,
    pub bound: u64,
    pub saturated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCapacityCounts {
    pub registrations: Option<CapacityCount>,
    pub retained_homes: Option<CapacityCount>,
    /// Exact retained reservation count, including when the home ceiling is unset.
    pub pending_launch_homes: u64,
    pub active_sessions: Option<CapacityCount>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdmissionCeilingsRead {
    pub state: AdmissionCeilingsState,
    pub history: Vec<AdmissionCeilingsChange>,
    pub effective_limits: AdmissionCeilings,
    pub counts: AdmissionCapacityCounts,
}

impl AdmissionCeilingsRead {
    pub fn validate(&self) -> Result<(), StoreError> {
        self.state.validate()?;
        if self.history.len()
            != self
                .state
                .revision
                .min(super::ADMISSION_HISTORY_LIMIT as u64) as usize
            || self.history.first() != self.state.last_change.as_ref()
            || self.history.windows(2).any(|pair| {
                pair[1].revision.checked_add(1) != Some(pair[0].revision)
                    || pair[0].old_ceilings != pair[1].new_ceilings
            })
        {
            return Err(StoreError::Malformed(
                "admission ceilings history is inconsistent".into(),
            ));
        }
        for change in &self.history {
            change.validate()?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait AdmissionCeilingsStore: Send + Sync {
    async fn read(&self) -> Result<AdmissionCeilingsState, StoreError>;
    async fn read_with_history(&self) -> Result<AdmissionCeilingsRead, StoreError>;
    async fn replace(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: AdmissionCeilings,
        now: u64,
    ) -> Result<AdmissionCeilingsState, StoreError>;
}

#[cfg(test)]
mod tests;
