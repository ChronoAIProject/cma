//! Bounded discovery. Catalog admission owns leases and transactional rechecks.
use std::{collections::BTreeSet, sync::Arc, time::Duration};

use layer2_core::sandbox::home_backup::{HomeBackupError, HomeBackupOrphanObservation};

pub(super) const PAGE_SIZE: u32 = 20;
pub(super) const MAX_SCAN_PAGES: usize = 4096;
const MAX_TOKEN_BYTES: usize = 4096;

#[derive(Clone)]
pub(crate) struct OrphanPage {
    pub entries: Vec<HomeBackupOrphanObservation>,
    pub next: Option<String>,
}

/// A native adapter must constrain every page to its own managed scope and the
/// backup labels, verify labels against immutable source/capture evidence, and
/// preserve the same selector across opaque continuation tokens. No unfiltered
/// inventory fallback is allowed. Conflict means the listing snapshot expired.
#[async_trait::async_trait]
pub(crate) trait OrphanInventory: Send + Sync {
    async fn list_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<OrphanPage, HomeBackupError>;
}

/// This operation may only perform idempotent catalog admission, not external
/// deletion. It must read authoritative tombstone status (unavailable => error),
/// recheck identity/state in the writing transaction, serialize against capture
/// and erasure, and use per-document lease/fence authority. Import requires full
/// original cut/source/custody evidence and the existing fixed retention policy.
/// Deletion requires durable expiring before effects. A timeout/unknown commit
/// can replay this same observation. Deleted replay is not yet implemented.
#[async_trait::async_trait]
pub(crate) trait OrphanCatalog: Send + Sync {
    async fn reconcile_observed(
        &self,
        observed: &HomeBackupOrphanObservation,
        now: &str,
    ) -> Result<(), HomeBackupError>;
}

pub(crate) struct OrphanSources {
    pub inventory: Arc<dyn OrphanInventory>,
    pub catalog: Arc<dyn OrphanCatalog>,
}

pub(crate) struct OrphanScanner {
    sources: OrphanSources,
    cursor: Option<String>,
    tokens: BTreeSet<String>,
    identities: BTreeSet<String>,
    pages: usize,
}

impl OrphanScanner {
    pub(crate) fn new(sources: OrphanSources) -> Self {
        Self {
            sources,
            cursor: None,
            tokens: BTreeSet::new(),
            identities: BTreeSet::new(),
            pages: 0,
        }
    }

    /// One page per worker tick, at most twenty admissions. Checkpoint only
    /// after the full page succeeds; partial page admission is replayed safely.
    /// No stored scan cursor or new persistence shape is introduced.
    pub(crate) async fn reconcile_page(&mut self, now: &str) -> Result<usize, HomeBackupError> {
        if self.pages >= MAX_SCAN_PAGES {
            self.reset();
            return Err(HomeBackupError::Invalid("orphan scan exceeds page bound"));
        }
        let page = tokio::time::timeout(
            Duration::from_secs(super::STEP_SECONDS),
            self.sources
                .inventory
                .list_page(self.cursor.as_deref(), PAGE_SIZE),
        )
        .await
        .map_err(|_| HomeBackupError::Unavailable)?;
        let page = match page {
            Err(error @ (HomeBackupError::Conflict | HomeBackupError::Invalid(_))) => {
                // An expired or invalid listing cannot safely resume using its
                // token. Restart next tick, retaining all durable admissions.
                self.reset();
                return Err(error);
            }
            result => result?,
        };
        let identities = match self.validate_page(&page, now) {
            Ok(identities) => identities,
            Err(error) => {
                self.reset();
                return Err(error);
            }
        };
        for entry in &page.entries {
            tokio::time::timeout(
                Duration::from_secs(super::STORE_SECONDS),
                self.sources.catalog.reconcile_observed(entry, now),
            )
            .await
            .map_err(|_| HomeBackupError::Unavailable)??;
        }
        let count = page.entries.len();
        if let Some(next) = page.next {
            self.tokens.insert(next.clone());
            self.identities.extend(identities);
            self.pages += 1;
            self.cursor = Some(next);
        } else {
            self.reset();
        }
        Ok(count)
    }

    fn validate_page(
        &self,
        page: &OrphanPage,
        now: &str,
    ) -> Result<BTreeSet<String>, HomeBackupError> {
        if page.entries.len() > PAGE_SIZE as usize {
            return Err(HomeBackupError::Invalid(
                "orphan inventory exceeds page limit",
            ));
        }
        if let Some(next) = &page.next {
            if next.is_empty()
                || next.len() > MAX_TOKEN_BYTES
                || next.chars().any(char::is_control)
                || self.tokens.contains(next)
            {
                return Err(HomeBackupError::Invalid(
                    "orphan inventory token did not advance",
                ));
            }
        }
        let mut identities = BTreeSet::new();
        for entry in &page.entries {
            entry.validate(now)?;
            if self.identities.contains(&entry.backup_id)
                || !identities.insert(entry.backup_id.clone())
            {
                return Err(HomeBackupError::Invalid(
                    "orphan inventory repeats a backup identity",
                ));
            }
        }
        Ok(identities)
    }

    fn reset(&mut self) {
        self.cursor = None;
        self.tokens.clear();
        self.identities.clear();
        self.pages = 0;
    }
}
