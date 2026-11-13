//! Optional scheduling presence; this hint never grants recovery authority.
use layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError;
use mongodb::bson::doc;
use std::time::Duration;

use super::agent_capacity_recovery::observations::{Observation, Phase};
use super::{agent_capacity_recovery::records::COLLECTION, MongoSandboxStore};

impl MongoSandboxStore {
    pub(crate) async fn capacity_recovery_work_exists(&self) -> Result<bool, RecoveryError> {
        let mut observed = Observation::new(Phase::Presence);
        let database = self.client.database(&self.agents.namespace().db);
        let names = tokio::time::timeout(Duration::from_secs(2), async {
            database
                .list_collection_names()
                .filter(doc! {"name": COLLECTION})
                .await
        })
        .await
        .map_err(|_| observed.deadline())?
        .map_err(|error| observed.driver(&error))?;
        let present = names.iter().any(|name| name == COLLECTION);
        if present {
            observed.complete();
        } else {
            observed.absent();
        }
        Ok(present)
    }
}
