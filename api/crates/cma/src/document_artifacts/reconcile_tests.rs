use super::*;
use layer2_core::document_artifacts::{BlobDeletion, BlobMaintenanceError, BlobPage};
use std::sync::Mutex;

#[derive(Default)]
struct Fixture {
    ledger: Vec<LedgerEntry>,
    references: Vec<ReferenceEntry>,
    ready: bool,
    delay: Duration,
    checked: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl ReconcileStore for Fixture {
    async fn ledger_page(&self, after: Option<&str>) -> Result<Vec<LedgerEntry>, DocumentError> {
        tokio::time::sleep(self.delay).await;
        Ok(self
            .ledger
            .iter()
            .filter(|row| after.is_none_or(|v| row.id.as_str() > v))
            .take(PAGE)
            .cloned()
            .collect())
    }
    async fn reference_page(
        &self,
        after: Option<&str>,
    ) -> Result<Option<ReferenceEntry>, DocumentError> {
        Ok(self
            .references
            .iter()
            .find(|row| after.is_none_or(|v| row.id.as_str() > v))
            .cloned())
    }
    async fn object_state(&self, key: &str) -> Result<Option<String>, DocumentError> {
        self.checked.lock().unwrap().push(key.into());
        Ok(self
            .ledger
            .iter()
            .find(|v| v.object_key.as_deref() == Some(key))
            .map(|v| v.state.clone().unwrap_or_else(|| "reserved".into())))
    }
    async fn referenced(&self, key: &str) -> Result<bool, DocumentError> {
        Ok(self
            .references
            .iter()
            .any(|v| v.keys.iter().any(|v| v == key)))
    }
    async fn ready(&self) -> Result<bool, DocumentError> {
        Ok(self.ready)
    }
}
struct Blobs {
    supported: bool,
    keys: Vec<String>,
    overlap_pages: bool,
    deleted: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl DocumentBlobMaintenance for Blobs {
    fn deletion_supported(&self) -> bool {
        self.supported
    }
    async fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        Ok(BlobPage {
            keys: self
                .keys
                .iter()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect(),
            next: (self.overlap_pages && cursor.is_none()).then(|| "next-page".into()),
        })
    }
    async fn delete(&self, key: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        self.deleted.lock().unwrap().push(key.into());
        Ok(BlobDeletion::Deleted)
    }
}
fn blobs(supported: bool, keys: Vec<String>) -> Blobs {
    Blobs {
        supported,
        keys,
        overlap_pages: false,
        deleted: Mutex::new(Vec::new()),
    }
}
fn owner() -> String {
    "a".repeat(64)
}
fn key() -> String {
    format!(
        "documents/{}/doc_cccccccccccccccccccccccccccccccc/{}.md",
        owner(),
        "b".repeat(64)
    )
}
fn owner_row(used: [u64; 3]) -> LedgerEntry {
    LedgerEntry {
        id: format!("owner:{}", owner()),
        owner: owner(),
        kind: "owner".into(),
        used: Some(used),
        baseline: Some(used),
        object_key: None,
        bytes: 0,
        state: None,
        since_ms: None,
    }
}
fn report() -> ReconcileReport {
    ReconcileReport::new(None, 3_600_000)
}
async fn finish(store: &Fixture, blobs: &Blobs, report: &mut ReconcileReport) {
    reconcile_until(
        store,
        Some(blobs),
        report,
        Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(report.complete);
}
#[tokio::test]
async fn reconcile_reports_each_invariant_and_overshoot_formula() {
    let store = Fixture {
        ledger: vec![owner_row([101, OWNER_DOCUMENT_BYTES + 1, 0])],
        ..Fixture::default()
    };
    let mut report = report();
    finish(&store, &blobs(true, vec![]), &mut report).await;
    let summary = report.summary();
    assert_eq!(summary["invariants"]["I1"], "FAIL");
    assert_eq!(summary["invariants"]["I2"], "PASS");
    assert_eq!(summary["invariants"]["I3"], "PASS");
    assert_eq!(summary["invariants"]["I4"], "NOT_CHECKED");
    assert_eq!(summary["invariants"]["I5"], "PASS");
    assert_eq!(summary["count_overshoot"], 1);
    assert_eq!(summary["bytes_overshoot"], 1);
    assert_eq!(summary["baseline_overshoot_owner_count"], 1);
}
#[tokio::test]
async fn reconcile_marks_storage_listing_not_checked_when_maintenance_disabled() {
    let mut report = report();
    let mut legacy_owner = owner_row([0; 3]);
    legacy_owner.baseline = None;
    finish(
        &Fixture {
            ledger: vec![legacy_owner],
            ..Fixture::default()
        },
        &blobs(false, vec![]),
        &mut report,
    )
    .await;
    assert_eq!(report.summary()["invariants"]["I4"], "NOT_CHECKED");
    assert!(report.summary()["orphan_blob_count"].is_null());
    assert_eq!(report.summary()["invariants"]["I5"], "PASS");
    assert_eq!(report.summary()["baseline_comparison"], "NOT_CHECKED");
}
#[tokio::test(start_paused = true)]
async fn reconcile_resumes_from_cursor_within_deadline() {
    let store = Fixture {
        ledger: vec![owner_row([0; 3])],
        delay: Duration::from_secs(3),
        ..Fixture::default()
    };
    let mut report = report();
    reconcile_until(
        &store,
        None,
        &mut report,
        Instant::now() + Duration::from_secs(4),
    )
    .await
    .unwrap();
    assert!(!report.complete);
    assert_eq!(
        report.cursor.ledger_after.as_deref(),
        Some(format!("owner:{}", owner()).as_str())
    );
    assert_eq!(report.summary()["invariants"]["I1"], "NOT_CHECKED");
    let serialized = serde_json::to_vec(&report).unwrap();
    let mut resumed: ReconcileReport = serde_json::from_slice(&serialized).unwrap();
    reconcile_until(
        &store,
        None,
        &mut resumed,
        Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(resumed.complete);
    assert_eq!(resumed.owners.len(), 1);
}
fn orphan_report() -> ReconcileReport {
    let mut report = report();
    report.orphans.push(key());
    report
}
#[tokio::test]
async fn orphan_cleanup_dry_run_changes_nothing() {
    let store = Fixture {
        ready: true,
        ..Fixture::default()
    };
    let blobs = blobs(true, vec![key()]);
    let result = cleanup(&store, &blobs, &orphan_report(), false)
        .await
        .unwrap();
    assert_eq!(result["eligible"], 1);
    assert_eq!(result["deleted"], 0);
    assert!(blobs.deleted.lock().unwrap().is_empty());
}
#[tokio::test]
async fn orphan_cleanup_refuses_without_ready_ledger() {
    let blobs = blobs(true, vec![key()]);
    assert!(cleanup(&Fixture::default(), &blobs, &orphan_report(), true)
        .await
        .is_err());
    assert!(blobs.deleted.lock().unwrap().is_empty());
}
#[tokio::test]
async fn orphan_cleanup_refuses_ledgered_or_referenced_keys() {
    let blobs = blobs(true, vec![key()]);
    let reference = ReferenceEntry {
        id: "doc_cccccccccccccccccccccccccccccccc".into(),
        owner: owner(),
        keys: vec![key()],
    };
    let store = Fixture {
        ready: true,
        references: vec![reference],
        ..Fixture::default()
    };
    assert!(cleanup(&store, &blobs, &orphan_report(), true)
        .await
        .is_err());
    let mut row = owner_row([0; 3]);
    row.kind = "object".into();
    row.object_key = Some(key());
    let store = Fixture {
        ready: true,
        ledger: vec![row],
        ..Fixture::default()
    };
    assert!(cleanup(&store, &blobs, &orphan_report(), true)
        .await
        .is_err());
    assert!(blobs.deleted.lock().unwrap().is_empty());
}
#[tokio::test]
async fn report_contains_no_subject_title_or_source() {
    let store = Fixture {
        ledger: vec![owner_row([0; 3])],
        ..Fixture::default()
    };
    let mut report = report();
    finish(&store, &blobs(true, vec![key()]), &mut report).await;
    let serialized = serde_json::to_string(&report).unwrap();
    for forbidden in ["owner_subject", "title", "markdown", "source", "subject"] {
        assert!(!serialized.contains(forbidden));
    }
    assert!(!serde_json::to_string(&report.summary())
        .unwrap()
        .contains(&key()));
    assert_eq!(report.orphans, vec![key()]);
}

#[tokio::test]
async fn reconcile_detects_aged_uncommitted_reference_and_exact_owner_totals() {
    let mut object = owner_row([0; 3]);
    object.id = format!(
        "object:{}:doc_{}:{}",
        owner(),
        "c".repeat(32),
        "b".repeat(64)
    );
    object.kind = "object".into();
    object.used = None;
    object.baseline = None;
    object.object_key = Some(key());
    object.bytes = 1;
    object.state = Some("reserved".into());
    object.since_ms = Some(0);
    let store = Fixture {
        ledger: vec![object, owner_row([0, 1, 1])],
        references: vec![ReferenceEntry {
            id: format!("doc_{}", "c".repeat(32)),
            owner: owner(),
            keys: vec![key()],
        }],
        ..Fixture::default()
    };
    let mut report = ReconcileReport::new(Some(owner()), 3_600_000);
    finish(&store, &blobs(true, vec![key()]), &mut report).await;
    let summary = report.summary();
    assert_eq!(summary["invariants"]["I1"], "PASS");
    assert_eq!(summary["invariants"]["I2"], "FAIL");
    assert_eq!(summary["invariants"]["I3"], "FAIL");
    assert_eq!(summary["invariants"]["I4"], "PASS");
    assert_eq!(summary["orphan_blob_count"], 0);
}
#[tokio::test]
async fn orphan_cleanup_apply_rechecks_and_deletes_only_proven_candidates() {
    let store = Fixture {
        ready: true,
        ..Fixture::default()
    };
    let blobs = blobs(true, vec![key()]);
    let result = cleanup(&store, &blobs, &orphan_report(), true)
        .await
        .unwrap();
    assert_eq!(result["deleted"], 1);
    assert_eq!(*blobs.deleted.lock().unwrap(), vec![key()]);
    assert_eq!(store.checked.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn reconcile_never_offers_referenced_unledgered_objects_for_cleanup() {
    let store = Fixture {
        ledger: vec![owner_row([0; 3])],
        references: vec![ReferenceEntry {
            id: format!("doc_{}", "c".repeat(32)),
            owner: owner(),
            keys: vec![key()],
        }],
        ..Fixture::default()
    };
    let mut report = ReconcileReport::new(Some(owner()), 3_600_000);
    finish(&store, &blobs(true, vec![key()]), &mut report).await;
    assert_eq!(report.summary()["orphan_blob_count"], 1);
    assert!(report.orphans.is_empty());
}

#[test]
fn reconcile_disabled_listing_never_reuses_a_persisted_orphan_failure() {
    let mut report = ReconcileReport::new(Some(owner()), 0);
    report.known_orphan_blob_count = 1;
    report.listing_supported = false;
    report.listing_complete = true;
    assert_eq!(report.summary()["invariants"]["I4"], "NOT_CHECKED");
    assert!(report.summary()["orphan_blob_count"].is_null());
}

#[tokio::test]
async fn reconcile_resumes_unchecked_listing_when_maintenance_becomes_available() {
    let store = Fixture {
        ledger: vec![owner_row([0; 3])],
        ..Fixture::default()
    };
    let mut report = ReconcileReport::new(Some(owner()), 0);
    finish(&store, &blobs(false, vec![]), &mut report).await;
    assert!(!report.listing_complete);
    finish(&store, &blobs(true, vec![key()]), &mut report).await;
    assert_eq!(report.summary()["orphan_blob_count"], 1);
    assert_eq!(report.orphans, vec![key()]);
}

#[test]
fn reconcile_resumed_age_evidence_is_not_checked_and_exposes_its_as_of_time() {
    let mut report = ReconcileReport::new(None, 42);
    report.ledger_complete = true;
    report.cursor.ledger_after = Some("owner:prior".into());
    report.note_resume();
    assert_eq!(report.summary()["invariants"]["I3"], "NOT_CHECKED");
    assert_eq!(report.summary()["age_as_of_ms"], 42);
}
#[tokio::test]
async fn reconcile_rejects_duplicate_keys_within_and_across_listing_pages() {
    for overlap in [false, true] {
        let store = Fixture {
            ledger: vec![owner_row([0; 3])],
            ..Fixture::default()
        };
        let mut report = ReconcileReport::new(Some(owner()), 0);
        let mut blobs = blobs(
            true,
            if overlap {
                vec![key()]
            } else {
                vec![key(), key()]
            },
        );
        blobs.overlap_pages = overlap;
        let result = reconcile_until(
            &store,
            Some(&blobs),
            &mut report,
            Instant::now() + Duration::from_secs(10),
        )
        .await;
        assert_eq!(result, Err(DocumentError::Corrupt));
        assert_eq!(report.orphans, vec![key()]);
        assert_eq!(report.known_orphan_blob_count, 1);
    }
}

#[tokio::test]
async fn reconcile_marks_future_reservation_timestamps_not_checked() {
    let mut object = owner_row([0; 3]);
    object.id = format!(
        "object:{}:doc_{}:{}",
        owner(),
        "c".repeat(32),
        "b".repeat(64)
    );
    object.kind = "object".into();
    object.object_key = Some(key());
    object.bytes = 1;
    object.state = Some("reserved".into());
    object.since_ms = Some(1);
    let store = Fixture {
        ledger: vec![object, owner_row([0, 1, 1])],
        ..Fixture::default()
    };
    let mut report = ReconcileReport::new(None, 0);
    finish(&store, &blobs(false, vec![]), &mut report).await;
    assert_eq!(report.summary()["invariants"]["I3"], "NOT_CHECKED");
    assert_eq!(report.unknown_age_rows, 1);
}
#[test]
fn reconcile_legacy_report_defaults_restart_only_untrusted_listing_evidence() {
    let mut report = ReconcileReport::new(None, 42);
    report.cursor.phase = 2;
    report.ledger_complete = true;
    report.known_orphan_blob_count = 1;
    report.orphans = vec![key()];
    let mut value = serde_json::to_value(report).unwrap();
    let fields = value.as_object_mut().unwrap();
    for key in [
        "aggregate",
        "baseline_overshoot_owners",
        "age_evidence_stale",
        "age_scan_completed_at_ms",
        "listing_cursor_version",
    ] {
        fields.remove(key);
    }
    let cursor = fields.get_mut("cursor").unwrap().as_object_mut().unwrap();
    cursor.remove("listing_seen_keys");
    cursor.remove("listing_seen_cursors");
    let mut resumed: ReconcileReport = serde_json::from_value(value).unwrap();
    resumed.note_resume();
    assert!(resumed.age_evidence_stale);
    assert_eq!(resumed.known_orphan_blob_count, 0);
    assert!(resumed.orphans.is_empty());
    assert_eq!(resumed.cursor.phase, 2);
    assert!(resumed.ledger_complete);
}
