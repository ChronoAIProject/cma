//! Purge one deleted document in two independently replayable, fenced passes.
use super::{deletion::valid_purge_object, *};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const WRITER_LEASE: Duration = Duration::from_secs(10 * 60);
pub const PURGE_LEASE: Duration = Duration::from_secs(90);
pub const PURGE_DELETE_CONCURRENCY: usize = 4;
pub const PURGE_ALERT_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const PURGE_KNOWN_BATCH: usize = PURGE_DELETE_CONCURRENCY;
pub const MAX_PURGE_PAGES: u64 = OWNER_DOCUMENT_OBJECT_LIMIT;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PurgePosition {
    KnownObjects {
        after: String,
    },
    ListedObjects {
        cursor: Option<String>,
        pages: u64,
    },
    ListedPage {
        remaining: Vec<String>,
        next: Option<String>,
        pages: u64,
    },
    Verify,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurgeClaimable {
    pub id: String,
    pub owner_hash: String,
    /// Unix milliseconds, on the persistence clock.
    pub deleted_at: i64,
    pub last_reserved_at: Option<i64>,
    pub pass: u8,
    pub next_at: i64,
    pub position: Option<PurgePosition>,
}
impl PurgeClaimable {
    pub fn prefix(&self) -> String {
        format!("documents/{}/{}/", self.owner_hash, self.id)
    }
    pub(super) fn validate(&self) -> Result<(), DocumentError> {
        if !valid_id(&self.id, "doc_")
            || !valid_digest(&self.owner_hash)
            || !matches!(self.pass, 1 | 2)
        {
            return Err(DocumentError::Corrupt);
        }
        let invalid_cursor = |value: &Option<String>| {
            value
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > MAX_BLOB_CURSOR_BYTES)
        };
        match &self.position {
            Some(PurgePosition::KnownObjects { after })
                if !valid_purge_object(&self.prefix(), after) =>
            {
                return Err(DocumentError::Corrupt)
            }
            Some(PurgePosition::ListedObjects { cursor, pages })
                if *pages > MAX_PURGE_PAGES || invalid_cursor(cursor) =>
            {
                return Err(DocumentError::Corrupt)
            }
            Some(PurgePosition::ListedPage {
                remaining,
                next,
                pages,
            }) if *pages == 0
                || *pages > MAX_PURGE_PAGES
                || remaining.is_empty()
                || remaining.len() > MAX_BLOB_PAGE_KEYS
                || remaining
                    .iter()
                    .any(|key| !valid_purge_object(&self.prefix(), key))
                || invalid_cursor(next) =>
            {
                return Err(DocumentError::Corrupt)
            }
            Some(PurgePosition::Verify) if self.pass != 2 => return Err(DocumentError::Corrupt),
            _ => (),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurgeLease {
    pub document: PurgeClaimable,
    pub holder: String,
    pub fence: u64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeStep {
    AwaitSecondPass {
        next_at: i64,
    },
    Retry {
        next_at: i64,
    },
    Progress {
        position: PurgePosition,
        next_at: i64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PurgeBacklog {
    pub pending_documents: u64,
    pub oldest_age_seconds: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurgeOutcome {
    pub claimed: u64,
    pub deleted: u64,
    pub released: u64,
    pub blocked: bool,
    pub blocked_reason: Option<&'static str>,
    pub retry_after: Option<Duration>,
    pub backlog: PurgeBacklog,
}

#[async_trait::async_trait]
pub trait DocumentPurgeStore: Send + Sync {
    /// Include blocked records when they are due so newly supported storage can resume.
    async fn due(&self, now: i64, limit: usize) -> Result<Vec<PurgeClaimable>, DocumentError>;
    /// Claim on the persistence clock for PURGE_LEASE, incrementing the fence.
    async fn claim(
        &self,
        id: &str,
        holder: &str,
        now: i64,
    ) -> Result<Option<PurgeLease>, DocumentError>;
    /// Merge the tombstone snapshot and ledger keys for this exact owner/document.
    /// May return only the next PURGE_KNOWN_BATCH+1 sorted unique keys after the
    /// known-object position. The extra key proves another batch remains.
    async fn objects(&self, lease: &PurgeLease) -> Result<Vec<String>, DocumentError>;
    /// Every mutation must match holder, fence and an unexpired lease. A stale
    /// holder is a no-op. Advance and block relinquish the current lease.
    async fn advance(&self, lease: &PurgeLease, step: PurgeStep) -> Result<bool, DocumentError>;
    /// Atomically mark complete and release ledger rows/counters exactly once.
    /// Unledgered documents still complete without inventing quota deductions.
    async fn release(&self, lease: &PurgeLease) -> Result<bool, DocumentError>;
    async fn block(&self, lease: &PurgeLease, reason: &str) -> Result<bool, DocumentError>;
    async fn backlog(&self, now: i64) -> Result<PurgeBacklog, DocumentError>;
}
