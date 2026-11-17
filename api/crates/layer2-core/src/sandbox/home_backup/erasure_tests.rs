mod service {
    use super::super::*;
    use crate::profiles::{
        AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
    };
    use crate::sandbox::home_backup::*;
    use crate::sandbox::store::{Page, PageRequest, SandboxStoreError};
    use crate::sandbox::{
        Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
        SandboxStatus, SandboxWorkState, SandboxWorkStep,
    };
    use crate::{SandboxId, SandboxProfileId};
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const HOLDER: &str = "deployment:00000000-0000-0000-0000-000000000001";
    const TOKEN: &str = "00000000-0000-0000-0000-000000000002";

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

    struct Backups {
        fenced: Mutex<Vec<String>>,
        fence_args: Mutex<Vec<(String, String, String)>>,
        ids: Vec<String>,
        records: Mutex<Vec<HomeBackup>>,
        fail_claim: bool,
        fail_save: bool,
        claim_attempts: AtomicUsize,
        save_attempts: AtomicUsize,
    }
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
            id: &str,
            now: &str,
            holder: &str,
            until: &str,
        ) -> Result<HomeBackup, HomeBackupError> {
            self.claim_attempts.fetch_add(1, Ordering::SeqCst);
            if self.fail_claim {
                return Err(HomeBackupError::Conflict);
            }
            let record = self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|v| v.id == id)
                .cloned()
                .ok_or(HomeBackupError::Conflict)?;
            record.claim_deletion(
                HomeBackupDeletionMode::Erasure,
                now,
                HomeBackupLease {
                    holder: holder.into(),
                    token: TOKEN.into(),
                    fence: record.next_fence,
                    expires_at: until.into(),
                },
            )
        }
        async fn save_deletion(
            &self,
            claimed: &HomeBackup,
            observation: &HomeBackupDeletionObservation,
            now: &str,
        ) -> Result<HomeBackup, HomeBackupError> {
            self.save_attempts.fetch_add(1, Ordering::SeqCst);
            if self.fail_save {
                return Err(HomeBackupError::Conflict);
            }
            claimed.complete_deletion(observation, now)
        }
    }
    #[async_trait]
    impl HomeBackupErasureStore for Backups {
        async fn fence_sandbox_erasure(
            &self,
            id: &str,
            now: &str,
            holder: &str,
            until: &str,
        ) -> Result<(), HomeBackupError> {
            self.fenced.lock().unwrap().push(id.into());
            self.fence_args
                .lock()
                .unwrap()
                .push((now.into(), holder.into(), until.into()));
            Ok(())
        }
        async fn list_sandbox_backups(
            &self,
            sandbox_id: &str,
        ) -> Result<Vec<String>, HomeBackupError> {
            Ok(self
                .ids
                .iter()
                .filter(|id| id.contains(sandbox_id))
                .cloned()
                .collect())
        }
    }
    struct Deletion {
        outcome: Mutex<Option<HomeBackupError>>,
        delay: Duration,
        calls: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl HomeBackupDeletionPort for Deletion {
        async fn delete_backup(
            &self,
            claimed: &HomeBackup,
            _: Duration,
        ) -> Result<HomeBackupDeletionObservation, HomeBackupError> {
            tokio::time::sleep(self.delay).await;
            if let Some(error) = self.outcome.lock().unwrap().take() {
                return Err(error);
            }
            self.calls.lock().unwrap().push(claimed.id.clone());
            Ok(HomeBackupDeletionObservation {
                snapshot_name: claimed.storage.snapshot_name.clone(),
                source_reference: claimed.storage.source_reference.clone(),
                absent: true,
                observed_at: "2026-11-17T00:00:00Z".into(),
                receipt: format!("receipt-{}", claimed.id),
            })
        }
    }
    struct Barrier {
        calls: AtomicUsize,
        outcome: Mutex<Option<HomeBackupError>>,
    }
    #[async_trait]
    impl OwnerErasureBarrier for Barrier {
        async fn admit_owner_erasure(&self, _: &SubjectId) -> Result<(), HomeBackupError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcome.lock().unwrap().take().map_or(Ok(()), Err)
        }
    }
    struct Clock {
        values: Mutex<VecDeque<String>>,
        fallback: String,
    }
    #[async_trait]
    impl HomeBackupClock for Clock {
        async fn now(&self) -> Result<String, HomeBackupError> {
            Ok(self
                .values
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| self.fallback.clone()))
        }
    }
    fn service(
        pages: Vec<Result<Page<Sandbox>, SandboxStoreError>>,
        backups: Arc<Backups>,
        deletion: Arc<Deletion>,
    ) -> HomeBackupErasureService {
        service_with(
            pages,
            backups,
            deletion,
            Arc::new(Barrier {
                calls: AtomicUsize::new(0),
                outcome: Mutex::new(None),
            }),
            Arc::new(Clock {
                values: Mutex::new(VecDeque::new()),
                fallback: "2026-11-17T00:00:00Z".into(),
            }),
        )
    }
    fn service_with(
        pages: Vec<Result<Page<Sandbox>, SandboxStoreError>>,
        backups: Arc<Backups>,
        deletion: Arc<Deletion>,
        barrier: Arc<Barrier>,
        clock: Arc<Clock>,
    ) -> HomeBackupErasureService {
        HomeBackupErasureService::new(
            Arc::new(Sandboxes(Mutex::new(pages.into_iter().collect()))),
            backups,
            deletion,
            barrier,
            clock,
        )
    }

    fn backups(ids: Vec<&str>, records: Vec<HomeBackup>) -> Arc<Backups> {
        backups_with_flags(ids, records, false, false)
    }

    fn backups_with_flags(
        ids: Vec<&str>,
        records: Vec<HomeBackup>,
        fail_claim: bool,
        fail_save: bool,
    ) -> Arc<Backups> {
        Arc::new(Backups {
            fenced: Mutex::new(vec![]),
            fence_args: Mutex::new(vec![]),
            ids: ids.into_iter().map(str::to_owned).collect(),
            records: Mutex::new(records),
            fail_claim,
            fail_save,
            claim_attempts: AtomicUsize::new(0),
            save_attempts: AtomicUsize::new(0),
        })
    }

    fn deletion(outcome: Option<HomeBackupError>, delay: Duration) -> Arc<Deletion> {
        Arc::new(Deletion {
            outcome: Mutex::new(outcome),
            delay,
            calls: Mutex::new(vec![]),
        })
    }
    fn page(items: Vec<Sandbox>, next: Option<&str>) -> Result<Page<Sandbox>, SandboxStoreError> {
        Ok(Page {
            items,
            next: next.map(str::to_owned),
        })
    }

    fn backup(id: &str, sandbox_id: &str) -> HomeBackup {
        HomeBackup {
            id: id.into(),
            sandbox_id: sandbox_id.into(),
            epoch: 1,
            source: HomeBackupSource {
                home_source_id: "source".into(),
                home_identity: "identity".into(),
                runtime_generation: 1,
                runtime_version: "runtime".into(),
                boot_id: "boot".into(),
            },
            cut: HomeBackupCut {
                requested_at: "2026-11-17T00:00:00Z".into(),
                source_creation_time: Some("2026-11-17T00:00:00Z".into()),
                ready_at: Some("2026-11-17T00:00:00Z".into()),
            },
            storage: HomeBackupStorage {
                snapshot_name: "snapshot".into(),
                snapshot_class: "retain".into(),
                source_reference: Some("handle".into()),
                restore_size_bytes: None,
            },
            custody: HomeBackupCustody {
                encryption_key_version_id: Some("key".into()),
                auth_key_ids: vec!["auth".into()],
            },
            retention: HomeBackupRetention {
                class: HomeBackupRetentionClass::Daily,
                expires_at: "2026-11-17T00:00:00Z".into(),
            },
            state: HomeBackupState::Ready,
            restore_pins: 0,
            lease: None,
            next_fence: 1,
            revision: 1,
            failure: None,
            erasure: None,
        }
    }

    #[tokio::test]
    async fn service_walks_multiple_pages_before_effects() {
        let backups = backups(
            vec!["hb-sbx-a-1", "hb-sbx-b-1"],
            vec![backup("hb-sbx-a-1", "sbx-a"), backup("hb-sbx-b-1", "sbx-b")],
        );
        let pages = vec![
            page(vec![sandbox("sbx-a", "owner")], Some("sbx-a")),
            page(vec![sandbox("sbx-b", "owner")], None),
            page(vec![sandbox("sbx-a", "owner")], Some("sbx-a")),
            page(vec![sandbox("sbx-b", "owner")], None),
        ];
        let deletion = deletion(None, Duration::ZERO);
        let receipts = service(pages, backups.clone(), deletion.clone())
            .erase_home_backups(
                &SubjectId::new("owner"),
                "2026-11-17T00:00:00Z",
                HOLDER,
                "2026-11-17T00:01:00Z",
            )
            .await
            .unwrap();
        assert_eq!(&*backups.fenced.lock().unwrap(), &["sbx-a", "sbx-b"]);
        assert_eq!(receipts.len(), 2);
        assert_eq!(
            &*deletion.calls.lock().unwrap(),
            &["hb-sbx-a-1", "hb-sbx-b-1"]
        );
    }

    #[tokio::test]
    async fn foreign_owner_and_changed_second_pass_block_all_effects() {
        let backups = backups(vec![], vec![]);
        let foreign = service(
            vec![page(vec![sandbox("sbx-x", "other")], None)],
            backups.clone(),
            deletion(None, Duration::ZERO),
        );
        assert!(matches!(
            foreign
                .erase_home_backups(
                    &SubjectId::new("owner"),
                    "2026-11-17T00:00:00Z",
                    HOLDER,
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
            deletion(None, Duration::ZERO),
        );
        assert!(matches!(
            changed
                .erase_home_backups(
                    &SubjectId::new("owner"),
                    "2026-11-17T00:00:00Z",
                    HOLDER,
                    "2026-11-17T00:01:00Z"
                )
                .await,
            Err(HomeBackupErasureError::InventoryChanged)
        ));
        assert!(backups.fenced.lock().unwrap().is_empty());
    }

    fn two_pass_pages(sandbox_id: &str) -> Vec<Result<Page<Sandbox>, SandboxStoreError>> {
        vec![
            page(vec![sandbox(sandbox_id, "owner")], None),
            page(vec![sandbox(sandbox_id, "owner")], None),
        ]
    }

    #[tokio::test]
    async fn invalid_holder_is_rejected_before_owner_barrier() {
        for holder in [
            "deployment-without-attempt",
            "deployment:not-a-uuid",
            "deployment:\u{0007}00000000-0000-0000-0000-000000000001",
        ] {
            let barrier = Arc::new(Barrier {
                calls: AtomicUsize::new(0),
                outcome: Mutex::new(None),
            });
            let backups = backups(vec![], vec![]);
            let service = service_with(
                two_pass_pages("sbx-a"),
                backups.clone(),
                deletion(None, Duration::ZERO),
                barrier.clone(),
                Arc::new(Clock {
                    values: Mutex::new(VecDeque::new()),
                    fallback: "2026-11-17T00:00:00Z".into(),
                }),
            );
            assert!(matches!(
                service
                    .erase_home_backups(
                        &SubjectId::new("owner"),
                        "2026-11-17T00:00:00Z",
                        holder,
                        "2026-11-17T00:01:00Z"
                    )
                    .await,
                Err(HomeBackupErasureError::Backup(HomeBackupError::Invalid(_)))
            ));
            assert_eq!(barrier.calls.load(Ordering::SeqCst), 0);
            assert!(backups.fenced.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn invalid_time_window_is_rejected_before_owner_barrier() {
        for (now, until) in [
            ("not-a-timestamp", "2026-11-17T00:01:00Z"),
            ("2026-11-17T00:01:00Z", "2026-11-17T00:00:00Z"),
        ] {
            let barrier = Arc::new(Barrier {
                calls: AtomicUsize::new(0),
                outcome: Mutex::new(None),
            });
            let backups = backups(vec![], vec![]);
            let result = service_with(
                two_pass_pages("sbx-a"),
                backups.clone(),
                deletion(None, Duration::ZERO),
                barrier.clone(),
                Arc::new(Clock {
                    values: Mutex::new(VecDeque::new()),
                    fallback: "2026-11-17T00:00:00Z".into(),
                }),
            )
            .erase_home_backups(&SubjectId::new("owner"), now, HOLDER, until)
            .await;
            assert!(matches!(
                result,
                Err(HomeBackupErasureError::Backup(HomeBackupError::Invalid(_)))
                    | Err(HomeBackupErasureError::Backup(HomeBackupError::Conflict))
            ));
            assert_eq!(barrier.calls.load(Ordering::SeqCst), 0);
            assert!(backups.fenced.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn unavailable_owner_barrier_fails_closed_before_inventory() {
        let barrier = Arc::new(Barrier {
            calls: AtomicUsize::new(0),
            outcome: Mutex::new(Some(HomeBackupError::Unavailable)),
        });
        let backups = backups(vec![], vec![]);
        let result = service_with(
            two_pass_pages("sbx-a"),
            backups.clone(),
            deletion(None, Duration::ZERO),
            barrier.clone(),
            Arc::new(Clock {
                values: Mutex::new(VecDeque::new()),
                fallback: "2026-11-17T00:00:00Z".into(),
            }),
        )
        .erase_home_backups(
            &SubjectId::new("owner"),
            "2026-11-17T00:00:00Z",
            HOLDER,
            "2026-11-17T00:01:00Z",
        )
        .await;
        assert!(matches!(
            result,
            Err(HomeBackupErasureError::DeletionBarrierUnavailable)
        ));
        assert_eq!(barrier.calls.load(Ordering::SeqCst), 1);
        assert!(backups.fenced.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn claim_failure_is_returned_without_provider_effect() {
        let backups = backups_with_flags(
            vec!["hb-sbx-a-1"],
            vec![backup("hb-sbx-a-1", "sbx-a")],
            true,
            false,
        );
        let deletion = deletion(None, Duration::ZERO);
        let result = service(two_pass_pages("sbx-a"), backups.clone(), deletion.clone())
            .erase_home_backups(
                &SubjectId::new("owner"),
                "2026-11-17T00:00:00Z",
                HOLDER,
                "2026-11-17T00:01:00Z",
            )
            .await;
        assert!(matches!(
            result,
            Err(HomeBackupErasureError::Backup(HomeBackupError::Conflict))
        ));
        assert_eq!(backups.claim_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(backups.save_attempts.load(Ordering::SeqCst), 0);
        assert!(deletion.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn provider_error_is_returned_without_save() {
        let backups = backups(vec!["hb-sbx-a-1"], vec![backup("hb-sbx-a-1", "sbx-a")]);
        let deletion = deletion(Some(HomeBackupError::Unavailable), Duration::ZERO);
        let result = service(two_pass_pages("sbx-a"), backups.clone(), deletion.clone())
            .erase_home_backups(
                &SubjectId::new("owner"),
                "2026-11-17T00:00:00Z",
                HOLDER,
                "2026-11-17T00:01:00Z",
            )
            .await;
        assert!(matches!(
            result,
            Err(HomeBackupErasureError::Backup(HomeBackupError::Unavailable))
        ));
        assert_eq!(backups.save_attempts.load(Ordering::SeqCst), 0);
        assert!(deletion.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn save_failure_reports_stolen_claim_after_provider_effect() {
        let backups = backups_with_flags(
            vec!["hb-sbx-a-1"],
            vec![backup("hb-sbx-a-1", "sbx-a")],
            false,
            true,
        );
        let deletion = deletion(None, Duration::ZERO);
        let result = service(two_pass_pages("sbx-a"), backups.clone(), deletion.clone())
            .erase_home_backups(
                &SubjectId::new("owner"),
                "2026-11-17T00:00:00Z",
                HOLDER,
                "2026-11-17T00:01:00Z",
            )
            .await;
        assert!(matches!(
            result,
            Err(HomeBackupErasureError::Backup(HomeBackupError::Conflict))
        ));
        assert_eq!(backups.claim_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(backups.save_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(&*deletion.calls.lock().unwrap(), &["hb-sbx-a-1"]);
    }

    #[tokio::test]
    async fn expired_claim_is_rejected_before_provider_effect() {
        let backups = backups(vec!["hb-sbx-a-1"], vec![backup("hb-sbx-a-1", "sbx-a")]);
        let clock = Arc::new(Clock {
            values: Mutex::new(
                [
                    "2026-11-17T00:00:00Z",
                    "2026-11-17T00:00:00Z",
                    "2026-11-17T00:00:03Z",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
            fallback: "2026-11-17T00:00:03Z".into(),
        });
        let deletion = deletion(None, Duration::ZERO);
        let result = service_with(
            two_pass_pages("sbx-a"),
            backups.clone(),
            deletion.clone(),
            Arc::new(Barrier {
                calls: AtomicUsize::new(0),
                outcome: Mutex::new(None),
            }),
            clock,
        )
        .erase_home_backups(
            &SubjectId::new("owner"),
            "2026-11-17T00:00:00Z",
            HOLDER,
            "2026-11-17T00:00:02Z",
        )
        .await;
        assert!(matches!(result, Err(HomeBackupErasureError::LeaseExpired)));
        assert!(deletion.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn provider_timeout_is_reported_before_save() {
        let backups = backups(vec!["hb-sbx-a-1"], vec![backup("hb-sbx-a-1", "sbx-a")]);
        let deletion = deletion(None, Duration::from_millis(1_100));
        let result = service(two_pass_pages("sbx-a"), backups.clone(), deletion)
            .erase_home_backups(
                &SubjectId::new("owner"),
                "2026-11-17T00:00:00Z",
                HOLDER,
                "2026-11-17T00:00:02Z",
            )
            .await;
        assert!(matches!(
            result,
            Err(HomeBackupErasureError::ProviderTimedOut)
        ));
        assert_eq!(backups.save_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn each_sandbox_fence_uses_a_fresh_clock_window() {
        let backups = backups(vec![], vec![]);
        let clock = Arc::new(Clock {
            values: Mutex::new(
                ["2026-11-17T00:00:00Z", "2026-11-17T00:01:00Z"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            ),
            fallback: "2026-11-17T00:01:00Z".into(),
        });
        let result = service_with(
            vec![
                page(vec![sandbox("sbx-a", "owner")], Some("a")),
                page(vec![sandbox("sbx-b", "owner")], None),
                page(vec![sandbox("sbx-a", "owner")], Some("a")),
                page(vec![sandbox("sbx-b", "owner")], None),
            ],
            backups.clone(),
            deletion(None, Duration::ZERO),
            Arc::new(Barrier {
                calls: AtomicUsize::new(0),
                outcome: Mutex::new(None),
            }),
            clock,
        )
        .erase_home_backups(
            &SubjectId::new("owner"),
            "2026-11-17T00:00:00Z",
            HOLDER,
            "2026-11-17T00:02:00Z",
        )
        .await
        .unwrap();
        assert!(result.is_empty());
        let fences = backups.fence_args.lock().unwrap();
        assert_eq!(fences.len(), 2);
        assert_eq!(fences[0].0, "2026-11-17T00:00:00Z");
        assert_eq!(fences[0].2, "2026-11-17T00:02:00Z");
        assert_eq!(fences[1].0, "2026-11-17T00:01:00Z");
        assert_eq!(fences[1].2, "2026-11-17T00:03:00Z");
    }
}
