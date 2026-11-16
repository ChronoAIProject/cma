use super::*;
use layer2_core::sandbox::home_backup::*;
use std::sync::Mutex;

struct Fixture {
    backup: Mutex<HomeBackup>,
    calls: Mutex<Vec<&'static str>>,
    outcome: &'static str,
}

impl Fixture {
    fn new(outcome: &'static str) -> Arc<Self> {
        Arc::new(Self {
            backup: Mutex::new(HomeBackup {
                id: "hb-expiry-fixture-1".into(),
                sandbox_id: "expiry-fixture".into(),
                epoch: 1,
                source: HomeBackupSource {
                    home_source_id: "source-fixture".into(),
                    home_identity: "home-fixture".into(),
                    runtime_generation: 1,
                    runtime_version: "fixture-image".into(),
                    boot_id: "fixture-boot".into(),
                },
                cut: HomeBackupCut {
                    requested_at: "2026-11-16T00:00:00Z".into(),
                    source_creation_time: Some("2026-11-16T00:00:01Z".into()),
                    ready_at: Some("2026-11-16T00:00:02Z".into()),
                },
                storage: HomeBackupStorage {
                    snapshot_name: "expiry-fixture".into(),
                    snapshot_class: "retain-fixture".into(),
                    source_reference: Some("copy-fixture".into()),
                    restore_size_bytes: None,
                },
                custody: HomeBackupCustody {
                    encryption_key_version_id: None,
                    auth_key_ids: vec![],
                },
                retention: HomeBackupRetention {
                    class: HomeBackupRetentionClass::Short,
                    expires_at: "2026-11-16T02:00:00Z".into(),
                },
                state: HomeBackupState::Ready,
                restore_pins: 0,
                lease: None,
                next_fence: 1,
                revision: 1,
                failure: None,
                erasure: None,
            }),
            calls: Mutex::new(Vec::new()),
            outcome,
        })
    }
    fn record(&self, call: &'static str) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait::async_trait]
impl HomeBackupDeletionStore for Fixture {
    async fn claim_expiring(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackup>, HomeBackupError> {
        self.record("resume");
        if self.backup.lock().unwrap().state != HomeBackupState::Expiring {
            return Ok(None);
        }
        self.claim_expiry(now, holder, until).await
    }
    async fn claim_expiry(
        &self,
        now: &str,
        holder: &str,
        until: &str,
    ) -> Result<Option<HomeBackup>, HomeBackupError> {
        self.record("claim");
        let mut value = self.backup.lock().unwrap();
        let until = if self.outcome == "short_lease" {
            layer2_core::rfc3339_at(std::time::SystemTime::now() + Duration::from_secs(24))
        } else {
            until.into()
        };
        let next = value.claim_deletion(
            HomeBackupDeletionMode::Expiry,
            now,
            HomeBackupLease {
                holder: holder.into(),
                token: uuid::Uuid::new_v4().to_string(),
                fence: value.next_fence,
                expires_at: until,
            },
        )?;
        *value = next.clone();
        Ok(Some(next))
    }
    async fn claim_erasure(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<HomeBackup, HomeBackupError> {
        unreachable!()
    }
    async fn save_deletion(
        &self,
        claimed: &HomeBackup,
        proof: &HomeBackupDeletionObservation,
        now: &str,
    ) -> Result<HomeBackup, HomeBackupError> {
        self.record("save");
        let mut stored = self.backup.lock().unwrap();
        assert_eq!(*stored, *claimed);
        let next = claimed.complete_deletion(proof, now)?;
        *stored = next.clone();
        Ok(next)
    }
}

#[async_trait::async_trait]
impl DeletionProvider for Fixture {
    async fn delete_copy(&self, claimed: &HomeBackup) -> Result<(), HomeBackupError> {
        self.record("delete");
        assert_eq!(self.backup.lock().unwrap().state, HomeBackupState::Expiring);
        assert_eq!(claimed.state, HomeBackupState::Expiring);
        if self.outcome == "delete_error" {
            Err(HomeBackupError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn observe_deletion(
        &self,
        claimed: &HomeBackup,
    ) -> Result<HomeBackupDeletionObservation, HomeBackupError> {
        self.record("observe");
        if self.outcome == "observe_error" {
            return Err(HomeBackupError::Unavailable);
        }
        Ok(HomeBackupDeletionObservation {
            snapshot_name: claimed.storage.snapshot_name.clone(),
            source_reference: claimed.storage.source_reference.clone(),
            absent: self.outcome != "present",
            observed_at: layer2_core::now_rfc3339(),
            receipt: "independent-provider-receipt-fixture".into(),
        })
    }
}

#[async_trait::async_trait]
impl orphans::OrphanInventory for Fixture {
    async fn list_page(
        &self,
        _: Option<&str>,
        _: u32,
    ) -> Result<orphans::OrphanPage, HomeBackupError> {
        self.record("list");
        if self.outcome == "inventory_error" {
            return Err(HomeBackupError::Unavailable);
        }
        Ok(orphans::OrphanPage {
            entries: vec![],
            next: None,
        })
    }
}

#[async_trait::async_trait]
impl orphans::OrphanCatalog for Fixture {
    async fn reconcile_observed(
        &self,
        _: &HomeBackupOrphanObservation,
        _: &str,
    ) -> Result<(), HomeBackupError> {
        panic!("empty inventory must not admit a catalog record")
    }
}

#[tokio::test]
async fn orphan_discovery_respects_admission_and_failure_leaves_deletion_recovery_available() {
    for (enabled, outcome) in [
        (false, "success"),
        (true, "success"),
        (true, "inventory_error"),
    ] {
        let fixture = Fixture::new(outcome);
        let worker = ExpiryWorker::new(
            enabled,
            fixture.clone(),
            Some(fixture.clone()),
            Some(orphans::OrphanSources {
                inventory: fixture.clone(),
                catalog: fixture.clone(),
            }),
            "expiry-fixture",
        )
        .unwrap();
        let result = worker.reconcile_orphan_page().await;
        assert_eq!(result.is_err(), outcome == "inventory_error");
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            if enabled { vec!["list"] } else { vec![] }
        );
        if outcome == "inventory_error" {
            assert!(worker.reconcile_one().await.unwrap());
            assert_eq!(
                fixture.backup.lock().unwrap().state,
                HomeBackupState::Deleted
            );
        }
    }
}

#[tokio::test]
async fn disabled_worker_performs_no_discovery_or_provider_calls() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(false, fixture.clone(), None, None, "expiry-fixture").unwrap();
    assert!(!worker.reconcile_one().await.unwrap());
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[test]
fn enabled_worker_requires_a_real_provider_boundary() {
    assert!(
        ExpiryWorker::new(true, Fixture::new("success"), None, None, "expiry-fixture").is_err()
    );
}

#[tokio::test]
async fn worker_persists_authority_then_deletes_observes_and_publishes_exact_receipt() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(
        true,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert!(worker.reconcile_one().await.unwrap());
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        vec!["claim", "delete", "observe", "save"]
    );
    assert_eq!(
        fixture.backup.lock().unwrap().state,
        HomeBackupState::Deleted
    );
}

#[tokio::test]
async fn provider_errors_or_present_copy_preserve_expiring_and_do_not_record_deletion() {
    for outcome in ["delete_error", "observe_error", "present"] {
        let fixture = Fixture::new(outcome);
        let worker = ExpiryWorker::new(
            true,
            fixture.clone(),
            Some(fixture.clone()),
            None,
            "expiry-fixture",
        )
        .unwrap();
        assert!(worker.reconcile_one().await.is_err());
        assert_eq!(
            fixture.backup.lock().unwrap().state,
            HomeBackupState::Expiring
        );
        assert!(fixture.backup.lock().unwrap().erasure.is_none());
        assert!(!fixture.calls.lock().unwrap().contains(&"save"));
    }
}

#[tokio::test]
async fn worker_refuses_an_external_step_when_the_remaining_lease_is_too_short() {
    let fixture = Fixture::new("short_lease");
    let worker = ExpiryWorker::new(
        true,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert_eq!(worker.reconcile_one().await, Err(HomeBackupError::Conflict));
    assert_eq!(*fixture.calls.lock().unwrap(), vec!["claim"]);
    assert_eq!(
        fixture.backup.lock().unwrap().state,
        HomeBackupState::Expiring
    );
}

#[tokio::test]
async fn disabled_supervision_heartbeats_and_shuts_down_without_store_work() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(false, fixture.clone(), None, None, "expiry-fixture").unwrap();
    let workers = crate::worker_supervision::Workers::new();
    let (shutdown, signal) = tokio::sync::watch::channel(false);
    let task = worker.spawn_supervised(signal, workers.get("home_backup_expiry"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(fixture.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn closing_admission_recovers_expiring_without_admitting_a_ready_backup() {
    let fixture = Fixture::new("success");
    let worker = ExpiryWorker::new(
        false,
        fixture.clone(),
        Some(fixture.clone()),
        None,
        "expiry-fixture",
    )
    .unwrap();
    assert!(!worker.reconcile_one().await.unwrap());
    assert_eq!(*fixture.calls.lock().unwrap(), vec!["resume"]);
    fixture.backup.lock().unwrap().state = HomeBackupState::Expiring;
    assert!(worker.reconcile_one().await.unwrap());
    assert_eq!(
        *fixture.calls.lock().unwrap(),
        vec!["resume", "resume", "claim", "delete", "observe", "save"]
    );
    assert_eq!(
        fixture.backup.lock().unwrap().state,
        HomeBackupState::Deleted
    );
}
