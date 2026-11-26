use super::{model::timestamp, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeBackupDeletionMode {
    Expiry,
    /// The store must also prove an erasing schedule in the claim transaction.
    Erasure,
}

/// Evidence from the independent deletion observer, after the delete request.
/// `absent` covers both the named object and its underlying retained copy.
/// This is an in-memory command; only the existing erasure receipt is persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupDeletionObservation {
    pub snapshot_name: String,
    pub source_reference: Option<String>,
    pub absent: bool,
    pub observed_at: String,
    pub receipt: String,
}

impl HomeBackup {
    pub fn claim_deletion(
        &self,
        mode: HomeBackupDeletionMode,
        now: &str,
        lease: HomeBackupLease,
    ) -> Result<Self, HomeBackupError> {
        self.validate()?;
        let at = timestamp(now)?;
        let eligible = match (mode, self.state) {
            (_, HomeBackupState::Expiring) => true,
            (HomeBackupDeletionMode::Expiry, HomeBackupState::Ready | HomeBackupState::Failed) => {
                timestamp(&self.retention.expires_at)? <= at
            }
            (HomeBackupDeletionMode::Erasure, state) => state != HomeBackupState::Deleted,
            _ => false,
        };
        if !eligible
            || self.restore_pins != 0
            || self.lease.as_ref().is_some_and(|previous| {
                timestamp(&previous.expires_at).is_ok_and(|expires| expires > at)
            })
            || lease.fence != self.next_fence
            || timestamp(&lease.expires_at)? <= at
        {
            return Err(HomeBackupError::Conflict);
        }
        let mut next = self.clone();
        next.state = HomeBackupState::Expiring;
        next.next_fence = increment(self.next_fence)?;
        next.revision = increment(self.revision)?;
        next.lease = Some(lease);
        next.validate()?;
        Ok(next)
    }

    pub fn complete_deletion(
        &self,
        observation: &HomeBackupDeletionObservation,
        now: &str,
    ) -> Result<Self, HomeBackupError> {
        self.validate()?;
        let at = timestamp(now)?;
        let observed = timestamp(&observation.observed_at)?;
        let lease = self.lease.as_ref().ok_or(HomeBackupError::Conflict)?;
        if self.state != HomeBackupState::Expiring
            || timestamp(&lease.expires_at)? <= at
            || !observation.absent
            || observation.snapshot_name != self.storage.snapshot_name
            || observation.source_reference != self.storage.source_reference
            || observed > at
            || observed < timestamp(&self.cut.requested_at)?
        {
            return Err(HomeBackupError::Conflict);
        }
        let mut next = self.clone();
        next.state = HomeBackupState::Deleted;
        next.revision = increment(self.revision)?;
        next.lease = None;
        next.erasure = Some(HomeBackupErasure {
            erased_at: observation.observed_at.clone(),
            receipt: observation.receipt.clone(),
        });
        next.validate()?;
        Ok(next)
    }
}
