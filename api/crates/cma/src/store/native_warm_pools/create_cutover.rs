//! Authorize an absence-only retirement after the deployment fences old creators.
use super::*;
use crate::store::{
    commit_transaction, start_transaction, transaction_terminal::GuardedTerminal as _,
    CommitOutcome,
};
use mongodb::bson::Bson;

impl MongoNativeWarmPools {
    fn create_cutover_authority(&self, epoch: uuid::Uuid) -> Document {
        // The deployment controller supplies this record after revoking
        // predecessor create authority and settling already dispatched creates.
        doc! {
            "_id": format!("native_pool_create_cutover:{}", self.deployment),
            "deployment": &self.deployment,
            "document_schema_version": crate::store::schema::DOCUMENT_SCHEMA_VERSION,
            "epoch": epoch.to_string(),
            "state": "fenced",
            "legacy_creates_settled": true,
        }
    }

    pub(super) async fn create_cutover_is_current(
        &self,
        epoch: uuid::Uuid,
    ) -> Result<bool, WarmPoolError> {
        Ok(
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS)
                .find_one(self.create_cutover_authority(epoch))
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
                .is_some(),
        )
    }

    pub(super) async fn retire_after_create_cutover(
        &self,
        target: &PoolTarget,
        epoch: uuid::Uuid,
    ) -> Result<(), WarmPoolError> {
        let settings =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let result = async {
            self.check_fenced_write(
                settings
                    .update_one(
                        self.create_cutover_authority(epoch),
                        doc! {"$inc": {"decision_fence": 1_i64}},
                    )
                    .session(&mut session)
                    .await,
            )
            .await?;
            let mut filter = fence(target);
            filter.extend(doc! {
                "retiring": true, "binding": Bson::Null, "target_replicas": 0_i64,
                // Missing means a legacy row covered by the deployment proof.
                // False means a newer create was dispatched and may still land.
                "$or": [
                    doc! {"provider_create_never_dispatched": true},
                    doc! {"provider_create_never_dispatched": {"$exists": false}},
                ],
            });
            self.check_fenced_write(
                self.targets
                    .update_one(filter, retirement::tombstone(target))
                    .session(&mut session)
                    .await,
            )
            .await
        }
        .await;
        if let Err(error) = result {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) => Err(WarmPoolError::Conflict),
            _ => Err(WarmPoolError::Unavailable),
        }
    }
}
