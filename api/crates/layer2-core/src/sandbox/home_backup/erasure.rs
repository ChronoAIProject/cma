//! Owner-scoped account erasure for independent home backups.

use super::{
    HomeBackupClock, HomeBackupDeletionPort, HomeBackupErasureReceipt, HomeBackupErasureStore,
    HomeBackupError, OwnerErasureBarrier,
};
use crate::{sandbox::store::SandboxStore, SubjectId};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum HomeBackupErasureError {
    #[error("sandbox ownership inventory unavailable: {0}")]
    Sandbox(#[source] crate::sandbox::store::SandboxStoreError),
    #[error("backup erasure unavailable: {0}")]
    Backup(#[source] HomeBackupError),
    #[error("sandbox ownership changed during erasure inventory")]
    InventoryChanged,
    #[error("sandbox inventory contains a foreign owner")]
    ForeignOwner,
    #[error("invalid account owner")]
    InvalidOwner,
    #[error("account deletion barrier is unavailable")]
    DeletionBarrierUnavailable,
    #[error("backup lease has expired before its provider effect")]
    LeaseExpired,
    #[error("provider deletion exceeded the remaining backup lease")]
    ProviderTimedOut,
}

pub struct HomeBackupErasureService {
    sandboxes: Arc<dyn SandboxStore>,
    backups: Arc<dyn HomeBackupErasureStore>,
    deletion: Arc<dyn HomeBackupDeletionPort>,
    barrier: Arc<dyn OwnerErasureBarrier>,
    clock: Arc<dyn HomeBackupClock>,
}

impl HomeBackupErasureService {
    pub fn new(
        sandboxes: Arc<dyn SandboxStore>,
        backups: Arc<dyn HomeBackupErasureStore>,
        deletion: Arc<dyn HomeBackupDeletionPort>,
        barrier: Arc<dyn OwnerErasureBarrier>,
        clock: Arc<dyn HomeBackupClock>,
    ) -> Self {
        Self {
            sandboxes,
            backups,
            deletion,
            barrier,
            clock,
        }
    }

    /// Enumerate the complete owner inventory, fence each schedule, delete
    /// every claimed provider copy, and return one receipt per backup. A
    /// second complete inventory pass detects a Sandbox added or removed while
    /// paging; callers retry instead of silently missing a newly-created
    /// schedule. The external effect is deliberately a separate port.
    pub async fn erase_home_backups(
        &self,
        subject: &SubjectId,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Vec<HomeBackupErasureReceipt>, HomeBackupErasureError> {
        if subject.as_str().is_empty() || subject.as_str().trim() != subject.as_str() {
            return Err(HomeBackupErasureError::InvalidOwner);
        }
        let start = layer2_core::seconds_from_rfc3339(now).ok_or(
            HomeBackupErasureError::Backup(HomeBackupError::Invalid("noncanonical timestamp")),
        )?;
        let deadline = layer2_core::seconds_from_rfc3339(until).ok_or(
            HomeBackupErasureError::Backup(HomeBackupError::Invalid("noncanonical timestamp")),
        )?;
        let lease_seconds = deadline
            .checked_sub(start)
            .filter(|seconds| *seconds > 1)
            .ok_or(HomeBackupErasureError::Backup(HomeBackupError::Conflict))?;
        if holder.is_empty() || holder.trim() != holder {
            return Err(HomeBackupErasureError::Backup(HomeBackupError::Invalid(
                "invalid erasure identity",
            )));
        }
        self.barrier
            .admit_owner_erasure(subject)
            .await
            .map_err(|_| HomeBackupErasureError::DeletionBarrierUnavailable)?;
        let first = self.inventory(subject).await?;
        let second = self.inventory(subject).await?;
        if first != second {
            return Err(HomeBackupErasureError::InventoryChanged);
        }
        let mut receipts = Vec::new();
        for sandbox_id in first {
            self.backups
                .fence_sandbox_erasure(&sandbox_id, now, holder, until)
                .await
                .map_err(HomeBackupErasureError::Backup)?;
            let backup_ids = self
                .backups
                .list_sandbox_backups(&sandbox_id)
                .await
                .map_err(HomeBackupErasureError::Backup)?;
            for backup_id in backup_ids {
                let claim_now = self.current_now().await?;
                let claim_until = lease_deadline(&claim_now, lease_seconds)?;
                let claimed = self
                    .backups
                    .claim_erasure(&backup_id, &claim_now, holder, &claim_until)
                    .await
                    .map_err(HomeBackupErasureError::Backup)?;
                let now_before_effect = self.current_now().await?;
                let expires = claimed
                    .lease
                    .as_ref()
                    .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
                    .ok_or(HomeBackupErasureError::LeaseExpired)?;
                let current = layer2_core::seconds_from_rfc3339(&now_before_effect)
                    .ok_or(HomeBackupErasureError::LeaseExpired)?;
                let remaining = expires.saturating_sub(current);
                if remaining <= 1 {
                    return Err(HomeBackupErasureError::LeaseExpired);
                }
                let observation = tokio::time::timeout(
                    Duration::from_secs(remaining - 1),
                    self.deletion
                        .delete_backup(&claimed, Duration::from_secs(remaining - 1)),
                )
                .await
                .map_err(|_| HomeBackupErasureError::ProviderTimedOut)?
                .map_err(HomeBackupErasureError::Backup)?;
                let settled_at = self.current_now().await?;
                let deleted = self
                    .backups
                    .save_deletion(&claimed, &observation, &settled_at)
                    .await
                    .map_err(HomeBackupErasureError::Backup)?;
                let erasure = deleted.erasure.ok_or(HomeBackupErasureError::Backup(
                    HomeBackupError::Invalid("deletion omitted receipt"),
                ))?;
                receipts.push(HomeBackupErasureReceipt {
                    sandbox_id: deleted.sandbox_id,
                    backup_id: deleted.id,
                    erased_at: erasure.erased_at,
                    receipt: erasure.receipt,
                });
            }
        }
        Ok(receipts)
    }

    async fn current_now(&self) -> Result<String, HomeBackupErasureError> {
        self.clock
            .now()
            .await
            .map_err(HomeBackupErasureError::Backup)
    }

    async fn inventory(&self, subject: &SubjectId) -> Result<Vec<String>, HomeBackupErasureError> {
        let mut cursor = None;
        let mut ids = BTreeSet::new();
        loop {
            let page = self
                .sandboxes
                .list_sandboxes(
                    subject,
                    &crate::sandbox::store::PageRequest { limit: 200, cursor },
                )
                .await
                .map_err(HomeBackupErasureError::Sandbox)?;
            for sandbox in page.items {
                insert_owned_sandbox(subject, sandbox.id.as_str(), &sandbox.owner, &mut ids)?;
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(ids.into_iter().collect())
    }
}

fn lease_deadline(now: &str, seconds: u64) -> Result<String, HomeBackupErasureError> {
    let at = layer2_core::seconds_from_rfc3339(now)
        .ok_or(HomeBackupErasureError::LeaseExpired)?
        .checked_add(seconds)
        .ok_or(HomeBackupErasureError::LeaseExpired)?;
    Ok(layer2_core::rfc3339_at(
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(at),
    ))
}

/// Functional entry point used by account-deletion orchestration code.
pub async fn erase_home_backups(
    service: &HomeBackupErasureService,
    subject: &SubjectId,
    now: &str,
    holder: &str,
    until: &str,
) -> Result<Vec<HomeBackupErasureReceipt>, HomeBackupErasureError> {
    service
        .erase_home_backups(subject, now, holder, until)
        .await
}

fn insert_owned_sandbox(
    subject: &SubjectId,
    sandbox_id: &str,
    owner: &SubjectId,
    ids: &mut BTreeSet<String>,
) -> Result<(), HomeBackupErasureError> {
    if owner != subject {
        return Err(HomeBackupErasureError::ForeignOwner);
    }
    ids.insert(sandbox_id.to_owned());
    Ok(())
}

#[cfg(test)]
#[path = "erasure_tests.rs"]
mod tests;
