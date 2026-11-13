//! Bounded orchestration for the fenced document purge lifecycle.
use super::{deletion::valid_purge_object, *};
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

impl DocumentMaintenance {
    pub fn with_purge(mut self, store: Arc<dyn DocumentPurgeStore>, enabled: bool) -> Self {
        self.purge = Some(store);
        self.purge_enabled = enabled;
        self
    }

    /// Process one known-object batch or one list page for one document.
    pub async fn purge_step(&self, now: i64) -> Result<PurgeOutcome, DocumentError> {
        let mut outcome = PurgeOutcome::default();
        let Some(store) = self.purge.as_ref().filter(|_| self.purge_enabled) else {
            outcome.blocked = true;
            outcome.blocked_reason = Some("purge_disabled");
            return Ok(outcome);
        };
        let clock = PurgeClock {
            now,
            started: tokio::time::Instant::now(),
        };
        let candidate = store.due(now, 1).await?.into_iter().next();
        if let Some(candidate) = candidate {
            candidate.validate()?;
            if let Some(lease) = store
                .claim(&candidate.id, &self.delete_holder, clock.now())
                .await?
            {
                lease.document.validate()?;
                if lease.document.id != candidate.id
                    || lease.document.owner_hash != candidate.owner_hash
                    || lease.holder != self.delete_holder
                {
                    return Err(DocumentError::Corrupt);
                }
                outcome.claimed = 1;
                if !self.blobs.deletion_supported() {
                    store.block(&lease, "storage_deletion_unsupported").await?;
                    outcome.blocked = true;
                    outcome.blocked_reason = Some("storage_deletion_unsupported");
                } else {
                    self.purge_claim(store.as_ref(), &lease, clock, &mut outcome)
                        .await?;
                }
            }
        }
        outcome.backlog = store.backlog(clock.now()).await?;
        Ok(outcome)
    }

    async fn purge_claim(
        &self,
        store: &dyn DocumentPurgeStore,
        lease: &PurgeLease,
        clock: PurgeClock,
        outcome: &mut PurgeOutcome,
    ) -> Result<(), DocumentError> {
        clock.check(lease)?;
        let writer_deadline = lease
            .document
            .last_reserved_at
            .unwrap_or(lease.document.deleted_at)
            .max(lease.document.deleted_at)
            .saturating_add(duration_millis(WRITER_LEASE));
        if lease.document.pass == 2 && clock.now() < writer_deadline {
            store
                .advance(
                    lease,
                    PurgeStep::Retry {
                        next_at: writer_deadline,
                    },
                )
                .await?;
            return Ok(());
        }
        let result = self.purge_batch(store, lease, clock, outcome).await;
        let position = match result {
            Ok(position) => position,
            Err(error) => {
                if matches!(error.error, DocumentError::Invalid | DocumentError::Corrupt) {
                    return Err(error.error);
                }
                let delay = error.retry_after.unwrap_or(RECLAIM_INTERVAL);
                outcome.retry_after = Some(delay);
                store
                    .advance(
                        lease,
                        PurgeStep::Retry {
                            next_at: clock.now().saturating_add(duration_millis(delay)),
                        },
                    )
                    .await?;
                return Ok(());
            }
        };
        clock.check(lease)?;
        if let Some(position) = position {
            store
                .advance(
                    lease,
                    PurgeStep::Progress {
                        position,
                        next_at: clock.now(),
                    },
                )
                .await?;
        } else if lease.document.pass == 1 {
            store
                .advance(
                    lease,
                    PurgeStep::AwaitSecondPass {
                        next_at: writer_deadline,
                    },
                )
                .await?;
        } else {
            outcome.released = u64::from(store.release(lease).await?);
        }
        Ok(())
    }

    async fn purge_batch(
        &self,
        store: &dyn DocumentPurgeStore,
        lease: &PurgeLease,
        clock: PurgeClock,
        outcome: &mut PurgeOutcome,
    ) -> Result<Option<PurgePosition>, BlobMaintenanceError> {
        match &lease.document.position {
            Some(PurgePosition::ListedObjects { cursor, pages }) => {
                self.purge_page(lease, cursor.as_deref(), *pages, clock)
                    .await
            }
            Some(PurgePosition::ListedPage {
                remaining,
                next,
                pages,
            }) => {
                let mut remaining = remaining.clone();
                let wave = remaining
                    .drain(..remaining.len().min(PURGE_DELETE_CONCURRENCY))
                    .collect();
                self.purge_delete(lease, wave, clock, outcome).await?;
                Ok(if !remaining.is_empty() {
                    Some(PurgePosition::ListedPage {
                        remaining,
                        next: next.clone(),
                        pages: *pages,
                    })
                } else if next.is_some() {
                    Some(PurgePosition::ListedObjects {
                        cursor: next.clone(),
                        pages: *pages,
                    })
                } else if lease.document.pass == 2 {
                    Some(PurgePosition::Verify)
                } else {
                    None
                })
            }
            Some(PurgePosition::Verify) => self.purge_page(lease, None, 0, clock).await,
            _ => {
                let prefix = lease.document.prefix();
                let objects: BTreeSet<_> = store.objects(lease).await?.into_iter().collect();
                if objects.iter().any(|key| !valid_purge_object(&prefix, key)) {
                    return Err(DocumentError::Corrupt.into());
                }
                let after = match &lease.document.position {
                    Some(PurgePosition::KnownObjects { after }) => Some(after.as_str()),
                    _ => None,
                };
                let mut keys: Vec<_> = objects
                    .into_iter()
                    .filter(|key| after.is_none_or(|after| key.as_str() > after))
                    .take(PURGE_KNOWN_BATCH + 1)
                    .collect();
                let more = keys.len() > PURGE_KNOWN_BATCH;
                keys.truncate(PURGE_KNOWN_BATCH);
                let position = if more {
                    PurgePosition::KnownObjects {
                        after: keys.last().expect("nonempty batch").clone(),
                    }
                } else {
                    PurgePosition::ListedObjects {
                        cursor: None,
                        pages: 0,
                    }
                };
                self.purge_delete(lease, keys, clock, outcome).await?;
                Ok(Some(position))
            }
        }
    }

    async fn purge_page(
        &self,
        lease: &PurgeLease,
        cursor: Option<&str>,
        pages: u64,
        clock: PurgeClock,
    ) -> Result<Option<PurgePosition>, BlobMaintenanceError> {
        if pages >= MAX_PURGE_PAGES {
            return Err(DocumentError::Corrupt.into());
        }
        let page = self.purge_list(lease, cursor, clock).await?;
        if page
            .next
            .as_deref()
            .is_some_and(|next| Some(next) == cursor)
        {
            return Err(DocumentError::Corrupt.into());
        }
        if page.keys.is_empty() {
            return Ok(if lease.document.pass == 2 && cursor.is_some() {
                Some(PurgePosition::Verify)
            } else {
                None
            });
        }
        // Persist the bounded page before deleting it. Its cursor may be opaque
        // and the provider need not return keys in lexical order.
        let remaining = page
            .keys
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Some(PurgePosition::ListedPage {
            remaining,
            next: page.next,
            pages: pages + 1,
        }))
    }

    async fn purge_list(
        &self,
        lease: &PurgeLease,
        cursor: Option<&str>,
        clock: PurgeClock,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        clock.check(lease)?;
        let prefix = lease.document.prefix();
        let page =
            tokio::time::timeout(clock.operation_budget()?, self.blobs.list(&prefix, cursor))
                .await
                .map_err(|_| DocumentError::Unavailable)??;
        if page.keys.len() > MAX_BLOB_PAGE_KEYS
            || page
                .keys
                .iter()
                .any(|key| !valid_purge_object(&prefix, key))
            || page.next.as_ref().is_some_and(|next| {
                next.len() > MAX_BLOB_CURSOR_BYTES || next.is_empty() || page.keys.is_empty()
            })
        {
            return Err(DocumentError::Corrupt.into());
        }
        Ok(page)
    }

    async fn purge_delete(
        &self,
        lease: &PurgeLease,
        objects: Vec<String>,
        clock: PurgeClock,
        outcome: &mut PurgeOutcome,
    ) -> Result<(), BlobMaintenanceError> {
        let mut objects = objects.into_iter();
        let mut inflight = FuturesUnordered::new();
        for key in objects.by_ref().take(PURGE_DELETE_CONCURRENCY) {
            inflight.push(self.purge_delete_one(lease, key, clock));
        }
        let mut failure: Option<BlobMaintenanceError> = None;
        while let Some(result) = inflight.next().await {
            match result {
                Ok(()) => outcome.deleted += 1,
                Err(error) => {
                    failure = Some(match failure {
                        Some(previous) => BlobMaintenanceError {
                            retry_after: previous.retry_after.max(error.retry_after),
                            ..previous
                        },
                        None => error,
                    });
                }
            }
            if failure.is_none() {
                if let Some(key) = objects.next() {
                    inflight.push(self.purge_delete_one(lease, key, clock));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn purge_delete_one(
        &self,
        lease: &PurgeLease,
        key: String,
        clock: PurgeClock,
    ) -> Result<(), BlobMaintenanceError> {
        clock.check(lease)?;
        tokio::time::timeout(clock.operation_budget()?, self.blobs.delete(&key))
            .await
            .map_err(|_| DocumentError::Unavailable)??;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct PurgeClock {
    now: i64,
    started: tokio::time::Instant,
}
impl PurgeClock {
    fn now(self) -> i64 {
        self.now
            .saturating_add(self.started.elapsed().as_millis().min(i64::MAX as u128) as i64)
    }
    fn operation_budget(self) -> Result<Duration, DocumentError> {
        // Leave time for the fenced progress write within the worker deadline.
        DELETE_OPERATION_TIMEOUT
            .checked_sub(self.started.elapsed())
            .and_then(|remaining| remaining.checked_sub(DELETE_LEASE_MARGIN))
            .filter(|remaining| !remaining.is_zero())
            .ok_or(DocumentError::Unavailable)
    }
    fn check(self, lease: &PurgeLease) -> Result<(), DocumentError> {
        if lease.expires_at.saturating_sub(self.now())
            <= duration_millis(DELETE_OPERATION_TIMEOUT + DELETE_LEASE_MARGIN)
        {
            return Err(DocumentError::Unavailable);
        }
        Ok(())
    }
}
fn duration_millis(value: Duration) -> i64 {
    value.as_millis().min(i64::MAX as u128) as i64
}
