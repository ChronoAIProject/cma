//! Read-only, resumable reconciliation and explicitly authorized external cleanup.
use layer2_core::document_artifacts::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::time::Instant;
#[path = "reconcile_io.rs"]
mod io;
#[path = "reconcile_mongo.rs"]
mod mongo;
#[path = "reconcile_scan.rs"]
mod scan;
pub(super) use io::execute;
const PAGE: usize = 64;
const MAX_REPORT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
struct LedgerEntry {
    id: String,
    owner: String,
    kind: String,
    used: Option<[u64; 3]>,
    baseline: Option<[u64; 3]>,
    object_key: Option<String>,
    bytes: u64,
    state: Option<String>,
    since_ms: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceEntry {
    id: String,
    owner: String,
    keys: Vec<String>,
}
#[async_trait::async_trait]
trait ReconcileStore: Send + Sync {
    async fn ledger_page(&self, after: Option<&str>) -> Result<Vec<LedgerEntry>, DocumentError>;
    async fn reference_page(
        &self,
        after: Option<&str>,
    ) -> Result<Option<ReferenceEntry>, DocumentError>;
    async fn object_state(&self, key: &str) -> Result<Option<String>, DocumentError>;
    async fn referenced(&self, key: &str) -> Result<bool, DocumentError>;
    async fn ready(&self) -> Result<bool, DocumentError>;
}
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerTotals {
    used: Option<[u64; 3]>,
    baseline: Option<[u64; 3]>,
    rows: [u64; 3],
}
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    phase: u8,
    ledger_after: Option<String>,
    reference_after: Option<String>,
    reference: Option<ReferenceEntry>,
    reference_index: usize,
    listing_owner: Option<String>,
    listing_cursor: Option<String>,
    listing_pending: Option<BlobKeys>,
    listing_index: usize,
    #[serde(default)]
    listing_seen_keys: BTreeSet<String>,
    #[serde(default)]
    listing_seen_cursors: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobKeys {
    keys: Vec<String>,
    next: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconcileReport {
    schema_version: u32,
    context: String,
    #[serde(default)]
    aggregate: serde_json::Value,
    owner: Option<String>,
    observed_at_ms: i64,
    #[serde(default)]
    age_evidence_stale: bool,
    #[serde(default)]
    age_scan_completed_at_ms: Option<i64>,
    complete: bool,
    ledger_complete: bool,
    references_complete: bool,
    listing_complete: bool,
    listing_supported: bool,
    #[serde(default)]
    listing_cursor_version: u32,
    listing_coverage: String,
    owners: BTreeMap<String, OwnerTotals>,
    #[serde(default)]
    baseline_overshoot_owners: Vec<String>,
    cursor: Cursor,
    missing_committed: u64,
    aged_rows: u64,
    unknown_age_rows: u64,
    known_orphan_blob_count: u64,
    orphans: Vec<String>,
}
impl ReconcileReport {
    fn new(owner: Option<String>, now_ms: i64) -> Self {
        let mut report = Self {
            schema_version: 1, context: String::new(), aggregate: serde_json::Value::Null,
            owner, observed_at_ms: now_ms, age_evidence_stale: false, age_scan_completed_at_ms: None, complete: false, ledger_complete: false,
            references_complete: false, listing_complete: false, listing_supported: false, listing_cursor_version: 1,
            listing_coverage: "Current logical objects for ledger/reference owners only; unknown owners and noncurrent versions are not enumerable".into(),
            owners: BTreeMap::new(), baseline_overshoot_owners: Vec::new(), cursor: Cursor::default(),
            missing_committed: 0, aged_rows: 0, unknown_age_rows: 0,
            known_orphan_blob_count: 0, orphans: Vec::new(),
        };
        report.aggregate = report.summary();
        report
    }
    fn note_resume(&mut self) {
        self.age_evidence_stale |= self.ledger_complete || self.cursor.ledger_after.is_some();
        if self.listing_cursor_version == 0 && self.cursor.phase >= 2 {
            // Older reports did not retain cross-page duplicate evidence. Restart
            // that read-only phase rather than trust its accumulated counts.
            self.known_orphan_blob_count = 0;
            self.orphans.clear();
            self.cursor.listing_owner = None;
            self.cursor.listing_cursor = None;
            self.cursor.listing_pending = None;
            self.cursor.listing_index = 0;
            self.cursor.listing_seen_keys.clear();
            self.cursor.listing_seen_cursors.clear();
            self.cursor.phase = 2;
            self.listing_complete = false;
            self.complete = false;
        }
        self.listing_cursor_version = 1;
    }
    fn summary(&self) -> serde_json::Value {
        let mut count_overshoot = 0_u64;
        let mut bytes_overshoot = 0_u64;
        let mut baseline_overshoot_owners = 0_u64;
        let mut i5_failed = false;
        let mut baseline_unknown = false;
        for totals in self.owners.values() {
            let baseline_exceeded = totals.baseline.is_some_and(|baseline| {
                baseline[0] > OWNER_DOCUMENT_LIMIT || baseline[1] > OWNER_DOCUMENT_BYTES
            });
            if baseline_exceeded {
                baseline_overshoot_owners += 1;
            }
            if let Some(used) = totals.used {
                count_overshoot =
                    count_overshoot.saturating_add(used[0].saturating_sub(OWNER_DOCUMENT_LIMIT));
                bytes_overshoot =
                    bytes_overshoot.saturating_add(used[1].saturating_sub(OWNER_DOCUMENT_BYTES));
                if baseline_exceeded {
                    i5_failed |= used
                        .iter()
                        .zip(totals.baseline.unwrap())
                        .any(|(used, baseline)| *used > baseline);
                } else if used[0] > OWNER_DOCUMENT_LIMIT || used[1] > OWNER_DOCUMENT_BYTES {
                    match totals.baseline {
                        Some(_) => i5_failed = true,
                        None => baseline_unknown = true,
                    }
                }
            } else {
                baseline_unknown = true;
            }
        }
        let baseline_missing = self
            .owners
            .values()
            .any(|v| v.used.is_some() && v.baseline.is_none());
        let i1_failed = self.owners.values().any(|v| v.used != Some(v.rows));
        let status = |failed: bool, checked: bool| {
            if failed {
                "FAIL"
            } else if checked {
                "PASS"
            } else {
                "NOT_CHECKED"
            }
        };
        serde_json::json!({
            "complete": self.complete,
            "invariants": {
                "I1": status(i1_failed && self.ledger_complete, self.ledger_complete),
                "I2": status(self.missing_committed > 0, self.references_complete),
                "I3": if self.age_evidence_stale { "NOT_CHECKED" } else { status(self.aged_rows > 0, self.ledger_complete && self.unknown_age_rows == 0) },
                "I4": if !self.listing_supported { "NOT_CHECKED" } else { status(self.known_orphan_blob_count > 0, self.listing_complete && self.owner.is_some()) },
                "I5": status(i5_failed, self.ledger_complete && !baseline_unknown)
            },
            "age_as_of_ms": self.observed_at_ms, "age_scan_completed_at_ms": self.age_scan_completed_at_ms,
            "age_evidence_stale": self.age_evidence_stale,
            "owner_count": self.owners.len(), "missing_committed_count": self.missing_committed,
            "aged_row_count": self.aged_rows, "unknown_age_row_count": self.unknown_age_rows,
            "count_overshoot": count_overshoot, "bytes_overshoot": bytes_overshoot,
            "baseline_overshoot_owner_count": baseline_overshoot_owners,
            "baseline_comparison": if baseline_unknown || baseline_missing { "NOT_CHECKED" } else { "CHECKED" },
            "orphan_blob_count": (self.listing_supported && self.listing_complete && self.owner.is_some()).then_some(self.known_orphan_blob_count),
            "observed_orphan_blob_count": self.listing_supported.then_some(self.known_orphan_blob_count),
            "cleanup_candidate_count": self.orphans.len(), "listing_coverage": self.listing_coverage
        })
    }
}

async fn reconcile_until(
    store: &dyn ReconcileStore,
    blobs: Option<&dyn DocumentBlobMaintenance>,
    report: &mut ReconcileReport,
    deadline: Instant,
) -> Result<(), DocumentError> {
    report.listing_supported = blobs.is_some_and(DocumentBlobMaintenance::deletion_supported);
    if report.listing_supported && !report.listing_complete && report.cursor.phase >= 3 {
        report.cursor.phase = 2;
        report.complete = false;
    }
    while !report.complete && Instant::now() < deadline {
        // A cancelled unit must not leave half-counted references or a cursor
        // ahead of its evidence. Each bounded unit publishes its report atomically.
        let mut next = report.clone();
        match tokio::time::timeout_at(deadline, scan::step(store, blobs, &mut next)).await {
            Ok(Ok(())) => {
                next.aggregate = next.summary();
                if serde_json::to_vec(&next)
                    .map_err(|_| DocumentError::Corrupt)?
                    .len()
                    > MAX_REPORT_BYTES / 2
                {
                    return Err(DocumentError::Capacity);
                }
                *report = next;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => break,
        }
    }
    Ok(())
}

async fn cleanup(
    store: &dyn ReconcileStore,
    blobs: &dyn DocumentBlobMaintenance,
    report: &ReconcileReport,
    apply: bool,
) -> Result<serde_json::Value, DocumentError> {
    if !store.ready().await? || !blobs.deletion_supported() {
        return Err(DocumentError::Unavailable);
    }
    let mut unique = std::collections::BTreeSet::new();
    for key in &report.orphans {
        let source = parse_key(key)?;
        if report
            .owner
            .as_ref()
            .is_some_and(|v| v != &source.owner_hash)
            || !unique.insert(key)
            || store.object_state(key).await?.is_some()
            || store.referenced(key).await?
        {
            return Err(DocumentError::Conflict);
        }
    }
    let mut deleted = 0_u64;
    if apply {
        for key in &report.orphans {
            // Source writers must be drained for this manual operation. These
            // fresh checks refuse keys that became owned after reconciliation.
            if !store.ready().await?
                || store.object_state(key).await?.is_some()
                || store.referenced(key).await?
            {
                return Err(DocumentError::Conflict);
            }
            blobs.delete(key).await.map_err(|error| error.error)?;
            deleted += 1;
        }
    }
    Ok(serde_json::json!({ "dry_run": !apply, "eligible": unique.len(), "deleted": deleted }))
}
fn parse_key(key: &str) -> Result<DocumentSourceReservation, DocumentError> {
    let parts: Vec<_> = key.split('/').collect();
    if let ["documents", owner, document, file] = parts.as_slice() {
        let source = DocumentSourceReservation {
            owner_hash: (*owner).into(),
            document_id: (*document).into(),
            sha256: file
                .strip_suffix(".md")
                .ok_or(DocumentError::Invalid)?
                .into(),
            bytes: 1,
        };
        source.validate()?;
        return Ok(source);
    }
    Err(DocumentError::Invalid)
}
#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
