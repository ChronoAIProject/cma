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

#[derive(Clone, Debug)]
pub struct ObjectCandidate {
    pub source: DocumentSourceReservation,
    /// Identifies one reservation incarnation when the object key is reused.
    pub reservation_id: Option<String>,
    /// Unix milliseconds; judgement must recheck this strict cutoff atomically.
    pub reserved_before: i64,
    pub condemned: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgement {
    Committed,
    Condemned,
    Skipped,
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
    /// Fence the owner, remove the condemned row and release counters atomically.
    async fn finalize(&self, candidate: &ObjectCandidate) -> Result<bool, DocumentError>;
    async fn backlog(&self, now: i64) -> Result<ReclaimBacklog, DocumentError>;
}
pub struct DocumentMaintenance {
    store: Arc<dyn DocumentReclaimStore>,
    blobs: Arc<dyn DocumentBlobMaintenance>,
    enabled: bool,
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
        }
    }
    /// Run one bounded batch. `now` is Unix milliseconds.
    pub async fn reclaim_step(&self, now: i64) -> Result<ReclaimOutcome, DocumentError> {
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
                Judgement::Skipped => (),
            }
        }
        self.delete_condemned(condemned, &mut outcome).await?;
        outcome.backlog = self.store.backlog(now).await?;
        Ok(outcome)
    }

    async fn delete_condemned(
        &self,
        mut candidates: VecDeque<ObjectCandidate>,
        outcome: &mut ReclaimOutcome,
    ) -> Result<(), DocumentError> {
        let mut inflight = FuturesUnordered::new();
        for candidate in candidates.drain(..candidates.len().min(RECLAIM_DELETE_CONCURRENCY)) {
            inflight.push(self.delete_candidate(candidate));
        }
        let mut stopped = false;
        let mut finalize_error = None;
        while let Some((candidate, result)) = inflight.next().await {
            match result {
                Ok(()) => {
                    // Both deletion and confirmed absence prove object finality.
                    outcome.deleted += 1;
                    match self.store.finalize(&candidate).await {
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
                    inflight.push(self.delete_candidate(candidate));
                }
            }
        }
        finalize_error.map_or(Ok(()), Err)
    }

    async fn delete_candidate(
        &self,
        candidate: ObjectCandidate,
    ) -> (ObjectCandidate, Result<(), BlobMaintenanceError>) {
        let result = self
            .blobs
            .delete(&candidate.source.object_key())
            .await
            .map(|_| ());
        (candidate, result)
    }
}
