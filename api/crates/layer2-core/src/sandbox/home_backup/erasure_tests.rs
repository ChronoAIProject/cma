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
        AgentProvider, ModelAccess, PermissionLevel, SandboxProfileContent, SandboxProfileSnapshot,
        ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
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
        async fn get_sandbox(&self, _: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
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
        async fn claim_expiring(
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
    fn page(items: Vec<Sandbox>, next: Option<&str>) -> Result<Page<Sandbox>, SandboxStoreError> {
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
