//! Reclaim reservations only after proving that immutable source objects are unused.
use super::{
    BlobMaintenanceError, DocumentBlobMaintenance, DocumentError, DocumentSourceReservation,
};
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::collections::VecDeque;
use std::{sync::Arc, time::Duration};

pub const RECLAIM_GRACE: Duration = Duration::from_secs(30 * 60);
pub const RECLAIM_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const RECLAIM_BATCH: usize = 64;
pub const RECLAIM_DELETE_CONCURRENCY: usize = 4;
pub const ORPHAN_ALERT_AGE: Duration = Duration::from_secs(24 * 60 * 60);
// Five minutes is deliberately much longer than the 30-second operation budget.
// Abandoned remote effects outliving this lease still require a finality contract.
pub const DELETE_LEASE: Duration = Duration::from_secs(5 * 60);
// The composed maintenance client must bound each delete by this budget.
pub const DELETE_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const DELETE_LEASE_MARGIN: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct ObjectCandidate {
    pub source: DocumentSourceReservation,
    /// Identifies one reservation incarnation when the object key is reused.
    pub reservation_id: Option<String>,
    /// Unix milliseconds; judgement must recheck this strict cutoff atomically.
    pub reserved_before: i64,
    pub condemned: bool,
}
#[derive(Clone, Debug)]
pub struct DeleteClaim {
    pub candidate: ObjectCandidate,
    pub holder: String,
    pub fence: u64,
    /// Unix milliseconds, on the same clock as the claim's `now` argument.
    pub lease_until: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgement {
    Committed,
    Condemned,
    Skipped,
    /// A lifecycle tombstone owns object deletion and the eventual quota release.
    OwnedByPurge,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReclaimBacklog {
    pub pending_objects: u64,
    pub oldest_age_seconds: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReclaimOutcome {
    pub judged: u64,
    pub committed: u64,
    pub condemned: u64,
    pub deleted: u64,
    pub released: u64,
    pub delete_failed: u64,
    pub blocked: bool,
    pub blocked_reason: Option<&'static str>,
    pub retry_after: Option<Duration>,
    pub backlog: ReclaimBacklog,
}
#[async_trait::async_trait]
pub trait DocumentReclaimStore: Send + Sync {
    /// True only while the quota switch is enabled and the ledger is ready.
    async fn ready(&self) -> Result<bool, DocumentError>;
    /// Return condemned rows first, followed by reservations older than the cutoff.
    async fn candidates(
        &self,
        reserved_before: i64,
        limit: usize,
    ) -> Result<Vec<ObjectCandidate>, DocumentError>;
    /// Read references and conditionally change the object state in one transaction.
    async fn judge(&self, candidate: &ObjectCandidate) -> Result<Judgement, DocumentError>;
    /// Exclusively claim an unleased or strictly expired condemned reservation.
    /// Refuse objects owned by a document purge, including previously condemned rows.
    async fn claim_delete(
        &self,
        candidate: &ObjectCandidate,
        holder: &str,
        now: i64,
    ) -> Result<Option<DeleteClaim>, DocumentError>;
    /// Match holder, fence and incarnation before releasing counters atomically.
    /// Recheck purge ownership so a concurrent document deletion retains its rows.
    async fn finalize(&self, claim: &DeleteClaim) -> Result<bool, DocumentError>;
    async fn backlog(&self, now: i64) -> Result<ReclaimBacklog, DocumentError>;
}
pub struct DocumentMaintenance {
    store: Arc<dyn DocumentReclaimStore>,
    pub(super) blobs: Arc<dyn DocumentBlobMaintenance>,
    enabled: bool,
    pub(super) delete_holder: String,
    pub(super) purge: Option<Arc<dyn super::DocumentPurgeStore>>,
    pub(super) purge_enabled: bool,
}
impl DocumentMaintenance {
    pub fn new(
        store: Arc<dyn DocumentReclaimStore>,
        blobs: Arc<dyn DocumentBlobMaintenance>,
        enabled: bool,
    ) -> Self {
        Self {
            store,
            blobs,
            enabled,
            purge: None,
            purge_enabled: false,
            // Constructed once for each maintenance worker during process startup.
            delete_holder: uuid::Uuid::new_v4().to_string(),
        }
    }
    /// Run one bounded batch. `now` is Unix milliseconds.
    pub async fn reclaim_step(&self, now: i64) -> Result<ReclaimOutcome, DocumentError> {
        let clock = ReclaimClock {
            now,
            started: tokio::time::Instant::now(),
        };
        // Capability gates precede ledger reads as well as all state transitions.
        let blocked_reason = if !self.enabled {
            Some("reclaim_disabled")
        } else if !self.blobs.deletion_supported() {
            Some("storage_deletion_unsupported")
        } else if !self.store.ready().await? {
            Some("ledger_not_ready")
        } else {
            None
        };
        if let Some(reason) = blocked_reason {
            return Ok(ReclaimOutcome {
                blocked: true,
                blocked_reason: Some(reason),
                ..Default::default()
            });
        }
        let cutoff = now.saturating_sub(RECLAIM_GRACE.as_millis() as i64);
        let candidates = self.store.candidates(cutoff, RECLAIM_BATCH).await?;
        let mut outcome = ReclaimOutcome::default();
        let mut condemned = VecDeque::new();
        for candidate in candidates.into_iter().take(RECLAIM_BATCH) {
            if candidate.condemned {
                condemned.push_back(candidate);
                continue;
            }
            outcome.judged += 1;
            match self.store.judge(&candidate).await? {
                Judgement::Committed => outcome.committed += 1,
                Judgement::Condemned => {
                    outcome.condemned += 1;
                    condemned.push_back(candidate);
                }
                Judgement::Skipped | Judgement::OwnedByPurge => (),
            }
        }
        self.delete_condemned(condemned, &mut outcome, clock)
            .await?;
        outcome.backlog = self.store.backlog(now).await?;
        Ok(outcome)
    }

    async fn delete_condemned(
        &self,
        mut candidates: VecDeque<ObjectCandidate>,
        outcome: &mut ReclaimOutcome,
        clock: ReclaimClock,
    ) -> Result<(), DocumentError> {
        let mut inflight = FuturesUnordered::new();
        for candidate in candidates.drain(..candidates.len().min(RECLAIM_DELETE_CONCURRENCY)) {
            inflight.push(self.delete_candidate(candidate, clock));
        }
        let mut stopped = false;
        let mut finalize_error = None;
        while let Some(attempt) = inflight.next().await {
            let (claim, result) = match attempt {
                Ok(Some(result)) => result,
                Ok(None) => {
                    if !stopped {
                        if let Some(candidate) = candidates.pop_front() {
                            inflight.push(self.delete_candidate(candidate, clock));
                        }
                    }
                    continue;
                }
                Err(error) => {
                    finalize_error.get_or_insert(error);
                    stopped = true;
                    continue;
                }
            };
            match result {
                Ok(()) => {
                    // Both deletion and confirmed absence prove object finality.
                    outcome.deleted += 1;
                    match self.store.finalize(&claim).await {
                        Ok(true) => outcome.released += 1,
                        Ok(false) => (),
                        Err(error) => {
                            finalize_error.get_or_insert(error);
                            stopped = true;
                        }
                    }
                }
                Err(error) => {
                    stopped = true;
                    outcome.delete_failed += 1;
                    let delay = error.retry_after.unwrap_or(RECLAIM_INTERVAL);
                    outcome.retry_after = Some(outcome.retry_after.unwrap_or_default().max(delay));
                }
            }
            // Once any operation fails, drain only work already started. Its
            // successful deletions can still be finalized; every other row stays
            // condemned and remains safe to revisit after interruption.
            if !stopped {
                if let Some(candidate) = candidates.pop_front() {
                    inflight.push(self.delete_candidate(candidate, clock));
                }
            }
        }
        finalize_error.map_or(Ok(()), Err)
    }

    async fn delete_candidate(
        &self,
        candidate: ObjectCandidate,
        clock: ReclaimClock,
    ) -> DeleteAttempt {
        let Some(claim) = self
            .store
            .claim_delete(&candidate, &self.delete_holder, clock.now())
            .await?
        else {
            return Ok(None);
        };
        // Include time spent judging, waiting for a claim and draining previous
        // completions. Do not send a request with an almost expired lease.
        if claim.lease_until.saturating_sub(clock.now())
            <= (DELETE_OPERATION_TIMEOUT + DELETE_LEASE_MARGIN).as_millis() as i64
        {
            return Ok(None);
        }
        let result = self
            .blobs
            .delete(&candidate.source.object_key())
            .await
            .map(|_| ());
        Ok(Some((claim, result)))
    }
}

type DeleteAttempt = Result<Option<(DeleteClaim, Result<(), BlobMaintenanceError>)>, DocumentError>;

#[derive(Clone, Copy)]
struct ReclaimClock {
    now: i64,
    started: tokio::time::Instant,
}
impl ReclaimClock {
    fn now(self) -> i64 {
        self.now
            .saturating_add(self.started.elapsed().as_millis().min(i64::MAX as u128) as i64)
    }
}
