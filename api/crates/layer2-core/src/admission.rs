//! Independent operator authority over new platform admissions.

use serde::{Deserialize, Serialize};

use crate::{budget::BudgetError, StoreError, SubjectId};

mod ceilings;
pub use ceilings::*;
mod history;
mod memory;
mod service;
pub use history::AdmissionControlRead;
pub use memory::MemoryAdmissionControlStore;
pub use service::AdmissionControlService;

pub const ADMISSION_PAUSED_GUIDANCE: &str =
    "New work is temporarily paused. Existing work continues.";
pub const MAX_ADMISSION_REASON_BYTES: usize = 512;
pub const ADMISSION_HISTORY_LIMIT: usize = 20;

/// A step of an operation durably admitted before the stop keeps its authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionCause {
    Fresh,
    Continuation,
}

/// Typed refusal shared by public admission projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionRefusal {
    #[error(transparent)]
    Capacity(#[from] CapacityKind),
    #[error(transparent)]
    Budget(#[from] BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    Paused,
}

impl AdmissionRefusal {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Budget(reason) => reason.code(),
            Self::Capacity(kind) => kind.code(),
            Self::Paused => "admission_paused",
        }
    }

    pub const fn guidance(&self) -> &'static str {
        match self {
            Self::Budget(reason) => reason.guidance(),
            Self::Capacity(kind) => kind.guidance(),
            Self::Paused => ADMISSION_PAUSED_GUIDANCE,
        }
    }
}

/// Additive top-level state; its revision never changes platform settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionControl {
    pub revision: u64,
    pub operator_stop: Option<OperatorStop>,
    pub last_change: Option<AdmissionChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorStop {
    pub since: u64,
    pub revision: u64,
    pub actor: SubjectId,
    pub reason: String,
}

/// Detailed evidence committed together with the control and activity record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionChange {
    pub revision: u64,
    pub at: u64,
    pub actor: SubjectId,
    pub reason: String,
    pub old_stop: Option<OperatorStop>,
    pub new_stop: Option<OperatorStop>,
}

impl AdmissionControl {
    pub fn validate(&self) -> Result<(), StoreError> {
        let invalid = || StoreError::Malformed("admission control is inconsistent".into());
        if self.revision > i64::MAX as u64 {
            return Err(invalid());
        }
        match &self.last_change {
            None if self.revision == 0 && self.operator_stop.is_none() => return Ok(()),
            Some(change)
                if change.revision == self.revision
                    && self.revision > 0
                    && change.new_stop == self.operator_stop
                    && valid_reason(&change.reason)
                    && !change.actor.as_str().is_empty()
                    && change.at <= i64::MAX as u64 => {}
            _ => return Err(invalid()),
        }
        if let Some(stop) = &self.operator_stop {
            let change = self.last_change.as_ref().ok_or_else(invalid)?;
            if stop.revision != self.revision
                || stop.since != change.at
                || stop.actor != change.actor
                || stop.reason != change.reason
            {
                return Err(invalid());
            }
        }
        let change = self.last_change.as_ref().ok_or_else(invalid)?;
        if change.old_stop.is_some() == change.new_stop.is_some() {
            return Err(invalid());
        }
        if let Some(previous) = &change.old_stop {
            if previous.revision.checked_add(1) != Some(self.revision)
                || previous.revision == 0
                || !valid_reason(&previous.reason)
                || previous.actor.as_str().is_empty()
                || previous.since > i64::MAX as u64
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// A duplicate stop returns the original writer's state regardless of the
    /// supplied revision. Every resume must match the current stop's revision.
    pub fn changed(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        stopped: bool,
        now: u64,
    ) -> Result<Option<Self>, StoreError> {
        self.validate()?;
        if stopped && self.operator_stop.is_some() {
            return Ok(None);
        }
        if self.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if !stopped && self.operator_stop.is_none() {
            return Ok(None);
        }
        if !valid_reason(reason) || actor.as_str().is_empty() || now > i64::MAX as u64 {
            return Err(StoreError::Malformed("admission change is invalid".into()));
        }
        let revision = self
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
            .ok_or_else(|| StoreError::Malformed("admission revision overflow".into()))?;
        let operator_stop = stopped.then(|| OperatorStop {
            since: now,
            revision,
            actor: actor.clone(),
            reason: reason.into(),
        });
        Ok(Some(Self {
            revision,
            last_change: Some(AdmissionChange {
                revision,
                at: now,
                actor: actor.clone(),
                reason: reason.into(),
                old_stop: self.operator_stop.clone(),
                new_stop: operator_stop.clone(),
            }),
            operator_stop,
        }))
    }
}

impl AdmissionChange {
    /// Existing activity vocabulary stays stable. Immutable history retains
    /// the full old/new state and reason alongside the activity record.
    pub fn activity_record(&self) -> Result<crate::ActivityRecord, StoreError> {
        let instant = std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(self.at))
            .ok_or_else(|| StoreError::Malformed("admission timestamp overflow".into()))?;
        let mut record = crate::ActivityRecord::new(
            crate::ActivityAction::Configure,
            crate::ActivityOutcome::Succeeded,
        )
        .by(self.actor.clone())
        .on(crate::ActivityTarget::Resource("admission_control".into()));
        record.at = crate::rfc3339_at(instant);
        record
            .detail
            .record(crate::DetailKey::Revision, self.revision.to_string());
        record.detail.record(
            crate::DetailKey::Status,
            if self.new_stop.is_some() {
                "stopped"
            } else {
                "open"
            },
        );
        Ok(record)
    }
}

pub(super) fn valid_reason(reason: &str) -> bool {
    !reason.trim().is_empty()
        && reason.len() <= MAX_ADMISSION_REASON_BYTES
        && !reason.chars().any(char::is_control)
}

/// Stores atomically persist the control and exactly one activity row for
/// each actual change; no-op requests append no activity record.
#[async_trait::async_trait]
pub trait AdmissionControlStore: Send + Sync {
    async fn read(&self) -> Result<AdmissionControl, StoreError>;
    /// Return control and the newest changes from one consistent snapshot.
    async fn read_with_history(&self) -> Result<AdmissionControlRead, StoreError>;
    async fn change(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        stopped: bool,
        now: u64,
    ) -> Result<AdmissionControl, StoreError>;
}

#[cfg(test)]
mod tests;
