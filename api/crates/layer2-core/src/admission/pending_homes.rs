//! Operator evidence for correcting the pending Launch home counter.

use serde::{Deserialize, Serialize};

use crate::{StoreError, SubjectId};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingHomesReconciliation {
    pub previous_pending_launch_homes: u64,
    pub expected_pending_launch_homes: u64,
    pub pending_launch_homes: u64,
    pub dry_run: bool,
    pub corrected: bool,
}

/// Immutable evidence is inserted in the same transaction as the counter and activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingHomesChange {
    pub at: u64,
    pub actor: SubjectId,
    pub reason: String,
    pub old_pending_launch_homes: u64,
    pub new_pending_launch_homes: u64,
}

impl PendingHomesChange {
    pub fn activity_record(&self) -> Result<crate::ActivityRecord, StoreError> {
        if !super::valid_reason(&self.reason)
            || self.actor.as_str().is_empty()
            || [
                self.at,
                self.old_pending_launch_homes,
                self.new_pending_launch_homes,
            ]
            .into_iter()
            .any(|value| value > i64::MAX as u64)
        {
            return Err(StoreError::Malformed(
                "invalid pending home correction".into(),
            ));
        }
        let instant = std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(self.at))
            .ok_or_else(|| StoreError::Malformed("pending home timestamp overflow".into()))?;
        let mut record = crate::ActivityRecord::new(
            crate::ActivityAction::Configure,
            crate::ActivityOutcome::Succeeded,
        )
        .by(self.actor.clone())
        .on(crate::ActivityTarget::Resource(
            "pending_launch_homes".into(),
        ));
        record.at = crate::rfc3339_at(instant);
        record.detail.record(crate::DetailKey::Status, "reconciled");
        Ok(record)
    }
}
