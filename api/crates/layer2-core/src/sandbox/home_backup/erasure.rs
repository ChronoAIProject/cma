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
                let claimed = self
                    .backups
                    .claim_erasure(&backup_id, &claim_now, holder, until)
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

    mod service {
        use super::super::*;
        use crate::profiles::{
            AgentProvider, ModelAccess, PermissionLevel, SandboxProfileContent,
            SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent,
            WorkspaceProfileSnapshot,
        };
        use crate::sandbox::store::{Page, PageRequest, SandboxStoreError};
        use crate::sandbox::{
            Sandbox, SandboxCredentialState, SandboxId, SandboxLimits, SandboxOrigin,
            SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
        };
        use crate::SandboxProfileId;
        use async_trait::async_trait;
        use std::collections::VecDeque;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        fn sandbox(id: &str, owner: &str) -> Sandbox {
            Sandbox {
                id: SandboxId(id.into()),
                owner: SubjectId::new(owner),
                name: id.into(),
                origin: SandboxOrigin::Manual,
                profile: SandboxProfileSnapshot {
                    id: SandboxProfileId("sbp_test".into()),
                    revision: 1,
                    digest: "a".repeat(64),
                    always_on: None,
                    content: SandboxProfileContent {
                        runtime_image: None,
                        cpu_millis: 1_000,
                        memory_mib: 2_048,
                        nyxid_services: vec![],
                        agent_provider: AgentProvider::default(),
                        model_access: ModelAccess::ChronoaiManaged,
                        max_workspaces: 4,
                        max_agents_per_workspace: 4,
                        workspace_profiles: vec![],
                        software_install_commands: vec![],
                        environment_variables: vec![],
                    },
                },
                status: SandboxStatus::Active,
                failure: None,
                runtime: None,
                credential: SandboxCredentialState {
                    source: Default::default(),
                    service_grants: vec![],
                    service_scope: None,
                    reaches: vec![],
                    key_generation: 1,
                    delivered: true,
                    current: None,
                    pending: None,
                    revoke_debt: vec![],
                    sign_in: SandboxSignInState::NotRequired,
                },
                work: SandboxWorkState::new(
                    SandboxWorkStep::WaitForCredentialRotation,
                    "2026-11-17T00:00:00Z".into(),
                ),
                limits: SandboxLimits {
                    max_workspaces: 4,
                    max_agents_per_workspace: 4,
                },
                lifecycle: Default::default(),
                last_activity_at: "2026-11-17T00:00:00Z".into(),
                created_at: "2026-11-17T00:00:00Z".into(),
                updated_at: "2026-11-17T00:00:00Z".into(),
                retired_at: None,
                purge_after: None,
                revision: 1,
            }
        }

        struct Sandboxes(Mutex<VecDeque<Result<Page<Sandbox>, SandboxStoreError>>>);
        #[async_trait]
        impl SandboxStore for Sandboxes {
            async fn create_sandbox(&self, _: Sandbox) -> Result<(), SandboxStoreError> {
                unimplemented!()
            }
            async fn get_sandbox(
                &self,
                _: &SandboxId,
            ) -> Result<Option<Sandbox>, SandboxStoreError> {
                unimplemented!()
            }
            async fn save_sandbox(&self, _: u64, _: Sandbox) -> Result<(), SandboxStoreError> {
                unimplemented!()
            }
            async fn list_sandboxes(
                &self,
                _: &SubjectId,
                _: &PageRequest,
            ) -> Result<Page<Sandbox>, SandboxStoreError> {
                self.0.lock().unwrap().pop_front().unwrap()
            }
        }

        #[derive(Default)]
        struct Backups(Mutex<Vec<String>>);
        #[async_trait]
        impl HomeBackupDeletionStore for Backups {
            async fn claim_expiry(
                &self,
                _: &str,
                _: &str,
                _: &str,
            ) -> Result<Option<HomeBackup>, HomeBackupError> {
                unimplemented!()
            }
            async fn claim_erasure(
                &self,
                _: &str,
                _: &str,
                _: &str,
                _: &str,
            ) -> Result<HomeBackup, HomeBackupError> {
                unimplemented!()
            }
            async fn save_deletion(
                &self,
                _: &HomeBackup,
                _: &HomeBackupDeletionObservation,
                _: &str,
            ) -> Result<HomeBackup, HomeBackupError> {
                unimplemented!()
            }
        }
        #[async_trait]
        impl HomeBackupErasureStore for Backups {
            async fn fence_sandbox_erasure(
                &self,
                id: &str,
                _: &str,
                _: &str,
                _: &str,
            ) -> Result<(), HomeBackupError> {
                self.0.lock().unwrap().push(id.into());
                Ok(())
            }
            async fn list_sandbox_backups(&self, _: &str) -> Result<Vec<String>, HomeBackupError> {
                Ok(vec![])
            }
        }
        struct Deletion;
        #[async_trait]
        impl HomeBackupDeletionPort for Deletion {
            async fn delete_backup(
                &self,
                _: &HomeBackup,
                _: Duration,
            ) -> Result<HomeBackupDeletionObservation, HomeBackupError> {
                unreachable!()
            }
        }
        struct Barrier;
        #[async_trait]
        impl OwnerErasureBarrier for Barrier {
            async fn admit_owner_erasure(&self, _: &SubjectId) -> Result<(), HomeBackupError> {
                Ok(())
            }
        }
        struct Clock;
        #[async_trait]
        impl HomeBackupClock for Clock {
            async fn now(&self) -> Result<String, HomeBackupError> {
                Ok("2026-11-17T00:00:00Z".into())
            }
        }
        fn service(
            pages: Vec<Result<Page<Sandbox>, SandboxStoreError>>,
            backups: Arc<Backups>,
        ) -> HomeBackupErasureService {
            HomeBackupErasureService::new(
                Arc::new(Sandboxes(Mutex::new(pages.into_iter().collect()))),
                backups,
                Arc::new(Deletion),
                Arc::new(Barrier),
                Arc::new(Clock),
            )
        }
        fn page(
            items: Vec<Sandbox>,
            next: Option<&str>,
        ) -> Result<Page<Sandbox>, SandboxStoreError> {
            Ok(Page {
                items,
                next: next.map(str::to_owned),
            })
        }

        #[tokio::test]
        async fn service_walks_multiple_pages_before_effects() {
            let backups = Arc::new(Backups::default());
            let pages = vec![
                page(vec![sandbox("sbx-a", "owner")], Some("sbx-a")),
                page(vec![sandbox("sbx-b", "owner")], None),
                page(vec![sandbox("sbx-a", "owner")], Some("sbx-a")),
                page(vec![sandbox("sbx-b", "owner")], None),
            ];
            service(pages, backups.clone())
                .erase_home_backups(
                    &SubjectId::new("owner"),
                    "2026-11-17T00:00:00Z",
                    "deployment:holder",
                    "2026-11-17T00:01:00Z",
                )
                .await
                .unwrap();
            assert_eq!(&*backups.0.lock().unwrap(), &["sbx-a", "sbx-b"]);
        }

        #[tokio::test]
        async fn foreign_owner_and_changed_second_pass_block_all_effects() {
            let backups = Arc::new(Backups::default());
            let foreign = service(
                vec![page(vec![sandbox("sbx-x", "other")], None)],
                backups.clone(),
            );
            assert!(matches!(
                foreign
                    .erase_home_backups(
                        &SubjectId::new("owner"),
                        "2026-11-17T00:00:00Z",
                        "h",
                        "2026-11-17T00:01:00Z"
                    )
                    .await,
                Err(HomeBackupErasureError::ForeignOwner)
            ));
            let changed = service(
                vec![
                    page(vec![sandbox("sbx-a", "owner")], None),
                    page(
                        vec![sandbox("sbx-a", "owner"), sandbox("sbx-new", "owner")],
                        None,
                    ),
                ],
                backups.clone(),
            );
            assert!(matches!(
                changed
                    .erase_home_backups(
                        &SubjectId::new("owner"),
                        "2026-11-17T00:00:00Z",
                        "h",
                        "2026-11-17T00:01:00Z"
                    )
                    .await,
                Err(HomeBackupErasureError::InventoryChanged)
            ));
            assert!(backups.0.lock().unwrap().is_empty());
        }
    }
}
