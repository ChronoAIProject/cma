use super::super::*;
use layer2_core::sandbox::home_backup::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

pub(super) struct Fixture {
    pub(super) backup: Mutex<HomeBackup>,
    pub(super) calls: Mutex<Vec<&'static str>>,
    pub(super) outcome: &'static str,
    pub(super) clock: Option<Arc<AtomicU64>>,
}

impl Fixture {
    pub(super) fn new(outcome: &'static str) -> Arc<Self> {
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
            clock: None,
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
        if self.outcome == "save_error" {
            return Err(HomeBackupError::Unavailable);
        }
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
            if self.outcome == "delete_clock_advance" {
                self.clock.as_ref().unwrap().fetch_add(35, Ordering::SeqCst);
            }
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
        match self.outcome {
            "observe_clock_to_save_boundary" => {
                self.clock.as_ref().unwrap().fetch_add(55, Ordering::SeqCst);
            }
            "observe_clock_with_save_budget" => {
                self.clock.as_ref().unwrap().fetch_add(54, Ordering::SeqCst);
            }
            _ => {}
        }
        Ok(HomeBackupDeletionObservation {
            snapshot_name: claimed.storage.snapshot_name.clone(),
            source_reference: claimed.storage.source_reference.clone(),
            absent: self.outcome != "present",
            observed_at: self
                .clock
                .as_ref()
                .map_or_else(layer2_core::now_rfc3339, |clock| {
                    layer2_core::rfc3339_at(
                        std::time::UNIX_EPOCH + Duration::from_secs(clock.load(Ordering::SeqCst)),
                    )
                }),
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
