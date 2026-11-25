//! Supervised, bounded recovery of durable Document storage effects.
use super::reclamation_store::{
    Candidate, ReclamationStore, ScanLane, DISCOVERY_INTERVAL, DISCOVERY_SLICE,
};
use super::MongoDocumentStore;
use crate::document_storage::maintenance::StorageMaintenance;
use layer2_core::document_artifacts::DocumentError;
use mongodb::bson::DateTime;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle, time::Instant};

const INTERVAL: Duration = Duration::from_secs(60);
const SLICE: Duration = Duration::from_secs(40);

pub(crate) struct ReclamationWorker {
    store: ReclamationStore,
    storage: Arc<StorageMaintenance>,
    retry_at: Instant,
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
            retry_at: Instant::now(),
        })
    }

    pub(crate) fn spawn(mut self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        let discovery = self.store.clone();
        tokio::spawn(async move {
            tokio::join!(
                discover(discovery, shutdown.clone()),
                self.effects(shutdown),
            );
        })
    }

    async fn effects(&mut self, shutdown: watch::Receiver<bool>) {
        let mut cadence = tokio::time::interval(INTERVAL);
        cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = crate::shutdown::signalled(shutdown.clone()) => break,
                _ = cadence.tick() => {}
            }
            tokio::select! {
                _ = crate::shutdown::signalled(shutdown.clone()) => break,
                result = tokio::time::timeout(SLICE, self.run_effects(DateTime::now())) => match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(%error, "document reclamation pass stopped"),
                    Err(_) => tracing::warn!("document reclamation pass reached its time budget"),
                }
            }
        }
    }

    pub(super) async fn run_effects(&mut self, now: DateTime) -> Result<(), DocumentError> {
        if Instant::now() < self.retry_at || !self.store.ready().await? {
            return Ok(());
        }
        let lane = ScanLane::Effect;
        let cursor = self.store.cursor(lane).await?;
        let page = self
            .store
            .expired_entries(cursor.after.as_deref(), now, 4)
            .await?;
        let mut next = None;
        let mut resolved = Vec::new();
        for entry in page {
            let outcome = match (entry.error, entry.candidate) {
                (Some(error), _) => Err(error),
                (None, Some(candidate)) => self.reclaim(&candidate, now).await,
                (None, None) => Ok(()),
            };
            match outcome {
                Ok(()) => resolved.push(entry.key.clone()),
                Err(error) => self.store.repair(lane, &entry.key, error, now).await?,
            }
            next = Some(entry.key);
            if Instant::now() < self.retry_at {
                break;
            }
        }
        self.store.resolved(lane, &resolved, now).await?;
        self.store.advance_cursor(lane, cursor, next).await?;
        Ok(())
    }

    async fn reclaim(&mut self, candidate: &Candidate, now: DateTime) -> Result<(), DocumentError> {
        let Some(claim) = self.store.claim(candidate, now).await? else {
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

async fn discover(store: ReclamationStore, shutdown: watch::Receiver<bool>) {
    let mut cadence = tokio::time::interval(DISCOVERY_INTERVAL);
    cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = crate::shutdown::signalled(shutdown.clone()) => break,
            _ = cadence.tick() => {}
        }
        tokio::select! {
            _ = crate::shutdown::signalled(shutdown.clone()) => break,
            result = tokio::time::timeout(DISCOVERY_SLICE, store.discover_page(DateTime::now())) => match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "document reclamation discovery pass stopped"),
                Err(_) => tracing::warn!("document reclamation discovery pass reached its time budget"),
            }
        }
    }
}
