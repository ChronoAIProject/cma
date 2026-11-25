//! Supervised, bounded recovery of durable Document storage effects.
use super::reclamation_store::{Candidate, ReclamationStore};
use super::MongoDocumentStore;
use crate::document_storage::maintenance::StorageMaintenance;
use layer2_core::document_artifacts::DocumentError;
use mongodb::bson::DateTime;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle, time::Instant};

const INTERVAL: Duration = Duration::from_secs(60);
const SLICE: Duration = Duration::from_secs(40);
const LEGACY_GRACE: Duration = Duration::from_secs(3600);

pub(crate) struct ReclamationWorker {
    store: ReclamationStore,
    storage: Arc<StorageMaintenance>,
    after: Option<String>,
    legacy_after: Instant,
    retry_at: Instant,
    accounting_first: bool,
}

impl ReclamationWorker {
    pub(crate) async fn prepare(
        documents: MongoDocumentStore,
        storage: Arc<StorageMaintenance>,
    ) -> Result<Self, DocumentError> {
        let store = ReclamationStore::new(documents, storage.namespace_sha256());
        store.ensure_indexes().await?;
        store.enable_generations().await?;
        Ok(Self {
            store,
            storage,
            after: None,
            legacy_after: Instant::now() + LEGACY_GRACE,
            retry_at: Instant::now(),
            accounting_first: true,
        })
    }

    pub(crate) fn spawn(mut self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut cadence = tokio::time::interval(INTERVAL);
            cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    _ = cadence.tick() => {}
                }
                let stop = async {
                    crate::shutdown::signalled(shutdown.clone()).await;
                };
                tokio::select! {
                    _ = stop => break,
                    result = tokio::time::timeout(SLICE, self.run_once()) => match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::warn!(%error, "document reclamation pass stopped"),
                        Err(_) => tracing::warn!("document reclamation pass reached its time budget"),
                    }
                }
            }
        })
    }

    pub(super) async fn run_once(&mut self) -> Result<(), DocumentError> {
        if Instant::now() < self.retry_at || !self.store.ready().await? {
            return Ok(());
        }
        let now = DateTime::now();
        let indexed = self.store.expired_claims(now).await?;
        let accounting = self
            .store
            .accounting_page(
                self.after.as_deref(),
                now,
                Instant::now() >= self.legacy_after,
            )
            .await?;
        if accounting.is_empty() {
            self.after = None;
        }
        let accounting = accounting
            .into_iter()
            .map(|entry| (Some(entry.key), entry.candidate));
        let indexed = indexed
            .into_iter()
            .take(4)
            .map(|candidate| (None, Some(candidate)));
        // Either lane can consume the pass deadline on a slow provider response.
        let work: Vec<_> = if self.accounting_first {
            accounting.chain(indexed).collect()
        } else {
            indexed.chain(accounting).collect()
        };
        self.accounting_first = !self.accounting_first;
        for (key, candidate) in work {
            if let Some(candidate) = candidate {
                self.reclaim(&candidate).await?;
            }
            if let Some(key) = key {
                self.after = Some(key);
            }
            if Instant::now() < self.retry_at {
                break;
            }
        }
        Ok(())
    }

    async fn reclaim(&mut self, candidate: &Candidate) -> Result<(), DocumentError> {
        let Some(claim) = self.store.claim(candidate, DateTime::now()).await? else {
            return Ok(());
        };
        match self
            .storage
            .delete_fenced(&claim.source.object_key(), claim.generation)
            .await
        {
            Ok(confirmation) => self.store.settle(&claim, confirmation).await?,
            Err(error) => {
                tracing::warn!(generation = claim.generation, error = %error,
                    "document storage confirmation is unresolved");
                if let Some(delay) = error.retry_after {
                    self.retry_at = Instant::now() + delay;
                }
            }
        }
        Ok(())
    }
}
