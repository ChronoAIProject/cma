//! Retained source custody and bounded projection, independent of transport.
mod capture;
mod completeness;
pub(crate) use completeness::{CompletenessAttempt, CompletenessReceipt, CompletenessResult};
mod delivery;
mod producer;
pub(crate) use delivery::{DeliveryAttempt, DeliveryOutcome};
mod inventory;
mod measured;
pub(crate) use measured::{retain_binding_bridge, run_repair_command};
mod outbox;
pub(crate) use outbox::FrozenUsageEvent;
mod payload;
mod project;
mod records;
pub(in crate::store) mod schema;
mod streams;
mod worker;

use super::{durability::majority_collection, sandbox_mongo::MongoSandboxStore};
use crate::config::UsageCaptureConfig;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{doc, Document},
    Client, Collection, Database,
};

pub(crate) const CAPTURES: &str = "sandbox_usage_captures_v1";
pub(crate) const EVIDENCE: &str = "sandbox_usage_evidence_v1";
pub(crate) const OUTBOX: &str = "sandbox_usage_outbox_v1";
pub(crate) const STREAMS: &str = "sandbox_usage_streams_v1";
pub(crate) const INVENTORY: &str = "sandbox_usage_inventory_v1";
pub(crate) const COLLECTIONS: &[&str] = &[CAPTURES, EVIDENCE, OUTBOX, STREAMS, INVENTORY];

#[derive(Clone)]
pub(crate) struct CaptureStore {
    client: Client,
    database: Database,
    config: UsageCaptureConfig,
}

impl CaptureStore {
    pub(crate) async fn open(
        client: &Client,
        database: &str,
        config: UsageCaptureConfig,
    ) -> Result<Self, SandboxStoreError> {
        let database = client.database(database);
        if schema::pending(&database).await.map_err(unavailable)? != 0 {
            return Err(unavailable(
                "usage capture schema requires explicit migration",
            ));
        }
        Ok(Self {
            client: client.clone(),
            database,
            config,
        })
    }

    pub(crate) fn namespace(&self) -> &str {
        &self.config.namespace
    }

    fn collection(&self, name: &str) -> Collection<Document> {
        majority_collection(&self.database, name)
    }
}

impl MongoSandboxStore {
    pub(crate) async fn with_usage_capture(
        mut self,
        config: Option<UsageCaptureConfig>,
    ) -> Result<Self, SandboxStoreError> {
        self.usage_capture = match config {
            None => None,
            Some(config) => Some(
                CaptureStore::open(&self.client, &self.sandboxes.namespace().db, config).await?,
            ),
        };
        Ok(self)
    }
}

fn unavailable(_: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable("allocated usage capture unavailable".into())
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("allocated usage evidence integrity conflict".into())
}

async fn retry_delay(attempt: usize) {
    if attempt > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(25 << (attempt - 1))).await;
    }
}

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod mongo_tests;
