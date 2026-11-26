//! Read-only reconciliation decisions; admission still needs durable authority.
use super::{model::timestamp, HomeBackup, HomeBackupError, HomeBackupState};

/// Fresh, identity-verified inventory evidence. This is not a persisted record
/// or sufficient metadata to manufacture a ready catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeBackupOrphanObservation {
    pub backup_id: String,
    pub sandbox_id: String,
    pub home_source_id: String,
    pub home_identity: String,
    pub snapshot_name: String,
    pub snapshot_class: String,
    pub snapshot_generation: String,
    pub source_reference: Option<String>,
    pub creation_time: Option<String>,
    pub ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeBackupOrphanDecision {
    Catalogued,
    AwaitEvidence,
    /// Recover the original metadata and apply the fixed retention policy.
    ImportByCreationTime,
    /// Persist deletion admission before effects; this is not a deletion receipt.
    AdmitDeletion,
}

impl HomeBackupOrphanObservation {
    pub fn validate(&self, now: &str) -> Result<(), HomeBackupError> {
        let now = timestamp(now)?;
        for value in [
            &self.backup_id,
            &self.sandbox_id,
            &self.home_source_id,
            &self.home_identity,
            &self.snapshot_name,
            &self.snapshot_class,
            &self.snapshot_generation,
        ]
        .into_iter()
        .chain(self.source_reference.as_ref())
        {
            if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
                return Err(HomeBackupError::Invalid("invalid orphan identity"));
            }
        }
        if let Some(created) = &self.creation_time {
            if timestamp(created)? > now {
                return Err(HomeBackupError::Invalid("orphan capture is in the future"));
            }
        }
        Ok(())
    }
}

/// The caller must obtain tombstone status from its authority, never infer it
/// from catalog absence. A writing adapter must repeat this decision in the
/// admission transaction, serialize with capture/erasure, and use document fences.
/// Existing deletion evidence is never converted back into a ready backup.
pub fn decide_orphan(
    observed: &HomeBackupOrphanObservation,
    catalogued: Option<&HomeBackup>,
    tombstoned: bool,
    now: &str,
) -> Result<HomeBackupOrphanDecision, HomeBackupError> {
    observed.validate(now)?;
    if let Some(known) = catalogued {
        known.validate()?;
        if known.id != observed.backup_id
            || known.sandbox_id != observed.sandbox_id
            || known.source.home_source_id != observed.home_source_id
            || known.source.home_identity != observed.home_identity
            || known.storage.snapshot_name != observed.snapshot_name
            || known.storage.snapshot_class != observed.snapshot_class
            || different(&known.storage.source_reference, &observed.source_reference)
            || different(&known.cut.source_creation_time, &observed.creation_time)
        {
            return Err(HomeBackupError::Conflict);
        }
    }
    if tombstoned
        || catalogued.is_some_and(|known| {
            matches!(
                known.state,
                HomeBackupState::Expiring | HomeBackupState::Deleted
            )
        })
    {
        return Ok(HomeBackupOrphanDecision::AdmitDeletion);
    }
    if catalogued.is_some() {
        return Ok(HomeBackupOrphanDecision::Catalogued);
    }
    if !observed.ready || observed.creation_time.is_none() || observed.source_reference.is_none() {
        return Ok(HomeBackupOrphanDecision::AwaitEvidence);
    }
    Ok(HomeBackupOrphanDecision::ImportByCreationTime)
}

fn different(previous: &Option<String>, observed: &Option<String>) -> bool {
    matches!((previous, observed), (Some(a), Some(b)) if a != b)
}
