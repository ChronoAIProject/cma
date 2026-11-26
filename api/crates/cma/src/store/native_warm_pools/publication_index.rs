//! Reserve the legacy activation key on the first authoritative pool target.
use super::*;
use crate::store::{commit_transaction, is_duplicate_key, CommitOutcome};
use mongodb::ClientSession;
use sha2::{Digest, Sha256};

impl MongoNativeWarmPools {
    pub(super) async fn verify_activation_floor(
        &self,
        machine: bool,
        image: &str,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let mut filter = retirement::release_scope(self, machine);
        filter.insert("durable_release_authority", true);
        if self
            .targets
            .find_one(filter)
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some()
        {
            self.verify_bootstrap_in(machine, image, Some(session))
                .await?;
        }
        Ok(())
    }

    pub(super) async fn prepare_publication_index(
        &self,
        machine: bool,
    ) -> Result<(), WarmPoolError> {
        let version = if machine { 2_i64 } else { 1_i64 };
        let mut legacy = doc! {
            "plan.deployment": &self.deployment, "document_schema_version":version,
            "activated_at":{"$type":"date"}, "ordered_activation":mongodb::bson::Bson::Null,
        };
        let index = IndexModel::builder()
            .keys(doc! {"plan.deployment":1, "document_schema_version":1})
            .options(
                IndexOptions::builder()
                    .name(format!(
                        "first_pointer_{version}_{:x}",
                        Sha256::digest(self.deployment.as_bytes())
                    ))
                    .unique(true)
                    .partial_filter_expression(legacy.clone())
                    .build(),
            )
            .build();

        // Classify only previously activated history, under the same transaction
        // fence as publication. The original anchor keeps the key through retirement.
        legacy.insert("first_pointer_anchor", doc! {"$ne":true});
        let mut session = self.order_activation(machine).await?;
        self.targets
            .update_many(legacy, doc! {"$set":{"ordered_activation":true}})
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => (),
            CommitOutcome::Retry(_) => return Err(WarmPoolError::Conflict),
            _ => return Err(WarmPoolError::Unavailable),
        }
        // On publication the chosen target occupies the single unclassified key.
        // A process already inside the legacy startup path still has to acquire
        // that same key when its update first writes activated_at on another target.
        self.targets.create_index(index).await.map_err(|error| {
            if is_duplicate_key(&error) {
                WarmPoolError::Conflict
            } else {
                WarmPoolError::Unavailable
            }
        })?;
        Ok(())
    }
}
