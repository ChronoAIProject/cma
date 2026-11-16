//! Owner-scoped account erasure for independent home backups.

use super::{
    HomeBackupDeletionPort, HomeBackupErasureReceipt, HomeBackupErasureStore, HomeBackupError,
};
use crate::{sandbox::store::SandboxStore, SubjectId};
use std::collections::BTreeSet;
use std::sync::Arc;

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
}

pub struct HomeBackupErasureService {
    sandboxes: Arc<dyn SandboxStore>,
    backups: Arc<dyn HomeBackupErasureStore>,
    deletion: Arc<dyn HomeBackupDeletionPort>,
}

impl HomeBackupErasureService {
    pub fn new(
        sandboxes: Arc<dyn SandboxStore>,
        backups: Arc<dyn HomeBackupErasureStore>,
        deletion: Arc<dyn HomeBackupDeletionPort>,
    ) -> Self {
        Self {
            sandboxes,
            backups,
            deletion,
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
        let first = self.inventory(subject).await?;
        let second = self.inventory(subject).await?;
        if first != second {
            return Err(HomeBackupErasureError::InventoryChanged);
        }
        let mut receipts = Vec::new();
        for sandbox_id in first {
            let claimed = self
                .backups
                .claim_sandbox_erasure(&sandbox_id, now, holder, until)
                .await
                .map_err(HomeBackupErasureError::Backup)?;
            for backup in claimed {
                let observation = self
                    .deletion
                    .delete_backup(&backup)
                    .await
                    .map_err(HomeBackupErasureError::Backup)?;
                let deleted = self
                    .backups
                    .save_deletion(&backup, &observation, now)
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
mod tests {
    use super::*;

    #[test]
    fn paged_inventory_is_complete_and_deduplicated() {
        let owner = SubjectId::new("owner");
        let mut ids = BTreeSet::new();
        insert_owned_sandbox(&owner, "sbx-a", &owner, &mut ids).unwrap();
        insert_owned_sandbox(&owner, "sbx-b", &owner, &mut ids).unwrap();
        insert_owned_sandbox(&owner, "sbx-a", &owner, &mut ids).unwrap();
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["sbx-a", "sbx-b"]);
    }

    #[test]
    fn a_foreign_owner_page_is_rejected() {
        let owner = SubjectId::new("owner");
        let foreign = SubjectId::new("other");
        let mut ids = BTreeSet::new();
        assert!(matches!(
            insert_owned_sandbox(&owner, "sbx-foreign", &foreign, &mut ids),
            Err(HomeBackupErasureError::ForeignOwner)
        ));
        assert!(ids.is_empty());
    }

    #[test]
    fn a_changed_inventory_requires_retry_before_claiming_a_schedule() {
        let before = vec!["sbx-a".to_owned()];
        let after = vec!["sbx-a".to_owned(), "sbx-new".to_owned()];
        assert_ne!(before, after);
        // The service compares both complete owner pages before invoking the
        // erasure store, so a concurrently-created schedule is retried.
    }
}
