//! In-memory contract fixture, not evidence of Mongo or provider capability.
use super::*;
use layer2_core::sandbox::home_backup::retention::{retain_ready_points, ReadyPoint};

pub(super) const NOW: &str = "2026-11-18T00:00:00Z";

pub(super) struct Fixture {
    pub pages: Mutex<BTreeMap<Option<String>, Result<OrphanPage, HomeBackupError>>>,
    pub requests: Mutex<Vec<(Option<String>, u32)>>,
    pub rows: Mutex<BTreeMap<String, HomeBackup>>,
    // Full immutable evidence is supplied explicitly by this fixture. Production
    // import may not infer missing source/custody/ready metadata from two labels.
    pub evidence: Mutex<BTreeMap<String, HomeBackup>>,
    pub tombstoned: Mutex<Result<bool, HomeBackupError>>,
    pub events: Mutex<Vec<&'static str>>,
}

impl Fixture {
    pub fn new(pages: Vec<Vec<HomeBackup>>) -> Arc<Self> {
        let count = pages.len();
        let mut evidence = BTreeMap::new();
        let pages = pages
            .into_iter()
            .enumerate()
            .map(|(index, rows)| {
                let entries = rows.iter().map(observation).collect();
                for row in rows {
                    evidence.insert(row.id.clone(), row);
                }
                let cursor = (index != 0).then(|| format!("page-{index}"));
                let next = (index + 1 < count).then(|| format!("page-{}", index + 1));
                (cursor, Ok(OrphanPage { entries, next }))
            })
            .collect();
        Arc::new(Self {
            pages: Mutex::new(pages),
            requests: Mutex::new(vec![]),
            rows: Mutex::new(BTreeMap::new()),
            evidence: Mutex::new(evidence),
            tombstoned: Mutex::new(Ok(false)),
            events: Mutex::new(vec![]),
        })
    }

    pub fn settle(&self, id: &str, absent: bool) -> Result<(), HomeBackupError> {
        let mut rows = self.rows.lock().unwrap();
        let claimed = rows.get(id).unwrap();
        assert_eq!(claimed.state, HomeBackupState::Expiring);
        self.events.lock().unwrap().extend(["delete", "observe"]);
        let observation = HomeBackupDeletionObservation {
            snapshot_name: claimed.storage.snapshot_name.clone(),
            source_reference: claimed.storage.source_reference.clone(),
            absent,
            observed_at: NOW.into(),
            receipt: "fixture-independent-copy-absence".into(),
        };
        let settled = claimed.complete_deletion(&observation, NOW)?;
        rows.insert(id.into(), settled);
        self.events.lock().unwrap().push("save");
        Ok(())
    }
}

#[async_trait::async_trait]
impl OrphanInventory for Fixture {
    async fn list_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<OrphanPage, HomeBackupError> {
        let cursor = cursor.map(str::to_owned);
        self.requests.lock().unwrap().push((cursor.clone(), limit));
        self.pages.lock().unwrap().get(&cursor).cloned().unwrap()
    }
}

#[async_trait::async_trait]
impl OrphanCatalog for Fixture {
    async fn reconcile_observed(
        &self,
        observed: &HomeBackupOrphanObservation,
        now: &str,
    ) -> Result<(), HomeBackupError> {
        let tombstoned = self.tombstoned.lock().unwrap().clone()?;
        let mut rows = self.rows.lock().unwrap();
        let known = rows.get(&observed.backup_id);
        match decide_orphan(observed, known, tombstoned, now)? {
            HomeBackupOrphanDecision::Catalogued | HomeBackupOrphanDecision::AwaitEvidence => {
                Ok(())
            }
            HomeBackupOrphanDecision::ImportByCreationTime => {
                let mut recovered = self
                    .evidence
                    .lock()
                    .unwrap()
                    .get(&observed.backup_id)
                    .cloned()
                    .ok_or(HomeBackupError::Unavailable)?;
                decide_orphan(observed, Some(&recovered), false, now)?;
                let history: Vec<_> = rows
                    .values()
                    .filter(|row| {
                        row.sandbox_id == recovered.sandbox_id
                            && row.source.home_source_id == recovered.source.home_source_id
                            && row.source.home_identity == recovered.source.home_identity
                    })
                    .chain(std::iter::once(&recovered))
                    .filter_map(|row| {
                        row.cut.ready_at.as_ref().map(|at| ReadyPoint {
                            backup_id: row.id.clone(),
                            ready_at: at.clone(),
                        })
                    })
                    .collect();
                let mut updated = rows.clone();
                for decision in retain_ready_points(&history)? {
                    if decision.backup_id == recovered.id {
                        recovered.retention = decision.retention;
                    } else if let Some(row) = updated
                        .get_mut(&decision.backup_id)
                        .filter(|row| row.state == HomeBackupState::Ready)
                    {
                        row.retention = decision.retention;
                        row.validate()?;
                    }
                }
                recovered.validate()?;
                updated.insert(recovered.id.clone(), recovered);
                *rows = updated;
                Ok(())
            }
            HomeBackupOrphanDecision::AdmitDeletion => {
                let previous = known
                    .cloned()
                    .or_else(|| {
                        self.evidence
                            .lock()
                            .unwrap()
                            .get(&observed.backup_id)
                            .cloned()
                    })
                    .ok_or(HomeBackupError::Unavailable)?;
                if previous.state == HomeBackupState::Expiring {
                    return Ok(());
                }
                // Deleted is intentionally refused by the existing model. The
                // production replay contract needs review, not a Ready reset.
                let admitted = previous.claim_deletion(
                    HomeBackupDeletionMode::Erasure,
                    now,
                    HomeBackupLease {
                        holder: format!("fixture-deployment:{}", uuid::Uuid::nil()),
                        token: uuid::Uuid::nil().to_string(),
                        fence: previous.next_fence,
                        expires_at: "2026-11-18T00:01:00Z".into(),
                    },
                )?;
                rows.insert(admitted.id.clone(), admitted);
                self.events.lock().unwrap().push("admit");
                Ok(())
            }
        }
    }
}

pub(super) fn scanner(fixture: &Arc<Fixture>) -> OrphanScanner {
    OrphanScanner::new(OrphanSources {
        inventory: fixture.clone(),
        catalog: fixture.clone(),
    })
}

pub(super) fn observation(row: &HomeBackup) -> HomeBackupOrphanObservation {
    HomeBackupOrphanObservation {
        backup_id: row.id.clone(),
        sandbox_id: row.sandbox_id.clone(),
        home_source_id: row.source.home_source_id.clone(),
        home_identity: row.source.home_identity.clone(),
        snapshot_name: row.storage.snapshot_name.clone(),
        snapshot_class: row.storage.snapshot_class.clone(),
        snapshot_generation: format!("generation-{}", row.epoch),
        source_reference: row.storage.source_reference.clone(),
        creation_time: row.cut.source_creation_time.clone(),
        ready: true,
    }
}

pub(super) fn candidate(epoch: u64, at: &str) -> HomeBackup {
    let seconds = layer2_core::seconds_from_rfc3339(at).unwrap();
    let value = HomeBackup {
        id: format!("hb-orphan-fixture-{epoch}"),
        sandbox_id: "orphan-fixture".into(),
        epoch,
        source: HomeBackupSource {
            home_source_id: "source-fixture".into(),
            home_identity: "home-fixture".into(),
            runtime_generation: 1,
            runtime_version: "image-fixture".into(),
            boot_id: "boot-fixture".into(),
        },
        // This fixture has complete original cut evidence. Equal timestamps here
        // do not authorize a production adapter to substitute creation for ready.
        cut: HomeBackupCut {
            requested_at: at.into(),
            source_creation_time: Some(at.into()),
            ready_at: Some(at.into()),
        },
        storage: HomeBackupStorage {
            snapshot_name: format!("orphan-copy-{epoch}"),
            snapshot_class: "retain-fixture".into(),
            source_reference: Some(format!("copy-fixture-{epoch}")),
            restore_size_bytes: None,
        },
        custody: HomeBackupCustody {
            encryption_key_version_id: Some("key-version-fixture".into()),
            auth_key_ids: vec!["auth-key-fixture".into()],
        },
        retention: HomeBackupRetention {
            class: HomeBackupRetentionClass::Daily,
            expires_at: layer2_core::rfc3339_at(
                std::time::UNIX_EPOCH + Duration::from_secs(seconds + 30 * 86400),
            ),
        },
        state: HomeBackupState::Ready,
        restore_pins: 0,
        lease: None,
        next_fence: 1,
        revision: 1,
        failure: None,
        erasure: None,
    };
    value.validate().unwrap();
    value
}
