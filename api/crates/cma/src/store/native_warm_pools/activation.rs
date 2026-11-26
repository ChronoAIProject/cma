//! Serialize fixed-pin activation with the first pointer publication for each lane.
use super::*;
use crate::store::released_runtimes::{BootWriter, MongoReleasedRuntimes};
use crate::store::{commit_transaction, is_confirmed_body_retry, CommitOutcome};
use layer2_core::released_runtime::ReleasedLaneRecord;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::ClientSession;

impl MongoNativeWarmPools {
    async fn order_activation(&self, machine: bool) -> Result<ClientSession, WarmPoolError> {
        // Fixed-pin writers can lack access to the optional released-runtime collections.
        // The required settings collection already hosts deployment coordination records.
        let guards =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        let key = doc! {"_id": {"native_pool_activation": &self.deployment, "machine": machine}};
        for attempt in 0..5 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << attempt)).await;
            }
            let mut session = self
                .database
                .client()
                .start_session()
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            crate::store::sandbox_mongo::start_transaction(&mut session)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            match guards
                .update_one(
                    key.clone(),
                    doc! {
                        "$setOnInsert": {"document_schema_version": 1_i64},
                        "$inc": {"decision_fence": 1_i64},
                    },
                )
                .upsert(true)
                .session(&mut session)
                .await
            {
                Ok(_) => return Ok(session),
                Err(error) if is_confirmed_body_retry(&error) => continue,
                Err(_) => return Err(WarmPoolError::Unavailable),
            }
        }
        Err(WarmPoolError::Conflict)
    }

    pub(super) async fn activate_target(&self, id: &str) -> Result<(), WarmPoolError> {
        let target = self
            .find(id)
            .await?
            .filter(|row| !row.retiring && !row.retired)
            .ok_or(WarmPoolError::Conflict)?;
        let image = &target.plan.spec.image.uri;
        if target.activated_at.is_some() {
            return self.verify_bootstrap(target.machine, image).await;
        }
        let mut session = self.order_activation(target.machine).await?;
        let scope = retirement::release_scope(self, target.machine);
        let mut floor = scope.clone();
        floor.insert("durable_release_authority", true);
        if self
            .targets
            .find_one(floor)
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some()
        {
            // Once a pointer owns the lane, only a qualified promotion can introduce
            // another image. A new sizing plan for the current image can still activate.
            self.verify_bootstrap_in(target.machine, image, Some(&mut session))
                .await?;
        }
        let mut historical = scope;
        historical.extend(doc! {"plan.spec.image.uri": image, "activated_at": {"$type": "date"}});
        let first = self
            .targets
            .find_one(historical)
            .sort(doc! {"activated_at": 1})
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let update = match first.and_then(|row| row.activated_at) {
            Some(at) => doc! {"$set": {"activated_at": at}},
            None => doc! {"$currentDate": {"activated_at": true}},
        };
        let mut filter = self.scope();
        filter.extend(doc! {"_id": id, "activated_at": mongodb::bson::Bson::Null, "retiring": {"$ne": true}, "retired": {"$ne": true}});
        self.targets
            .update_one(filter, update)
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let mut live = self.scope();
        live.extend(doc! {"_id": id, "activated_at": {"$type": "date"}, "retired": {"$ne": true}, "retiring": {"$ne": true}});
        if self
            .targets
            .find_one(live)
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_none()
        {
            return Err(WarmPoolError::Conflict);
        }
        // Keep the original release timestamp of a historical plan even if it is
        // no longer eligible. The final read refuses superseded or ambiguous pins.
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => (),
            CommitOutcome::Unknown(_)
                if self.find(id).await?.is_some_and(|row| {
                    row.plan == target.plan
                        && !row.retired
                        && !row.retiring
                        && row.activated_at.is_some()
                }) =>
            {
                ()
            }
            CommitOutcome::Retry(_) => return Err(WarmPoolError::Conflict),
            _ => return Err(WarmPoolError::Unavailable),
        }
        self.verify_bootstrap(target.machine, image).await
    }

    pub(crate) async fn publish_bootstrap(
        &self,
        target: &PoolTarget,
        lanes: &MongoReleasedRuntimes,
        writer: &BootWriter,
        record: &ReleasedLaneRecord,
    ) -> Result<(), WarmPoolError> {
        let mut session = self.order_activation(target.machine).await?;
        self.verify_bootstrap_in(target.machine, &record.active.image, Some(&mut session))
            .await?;
        let mut filter = self.scope();
        filter.extend(doc! {"_id": &target.id, "plan.spec.image.uri": &record.active.image, "activated_at": {"$type": "date"},
            "retiring": {"$ne": true}, "retired": {"$ne": true}});
        self.check_fenced_write(
            self.targets
                .update_one(filter, doc! {"$set": {"durable_release_authority": true}})
                .session(&mut session)
                .await,
        )
        .await?;
        lanes
            .protect_boot_effect_in(writer, Some(&mut session))
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        lanes
            .insert_bootstrap(record, Some(&mut session))
            .await
            .map_err(|error| match error {
                SandboxStoreError::Conflict => WarmPoolError::Conflict,
                _ => WarmPoolError::Unavailable,
            })?;
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            // Re-enter the controller's bounded lane read on a confirmed abort
            // or a majority-visible competing bootstrap. Never replay an unknown write.
            CommitOutcome::Retry(_) => Err(WarmPoolError::Conflict),
            CommitOutcome::Unknown(_) => match lanes.load(record.lane).await {
                Ok(Some(current)) if current == *record => Ok(()),
                Ok(Some(_)) => Err(WarmPoolError::Conflict),
                _ => Err(WarmPoolError::Unavailable),
            },
            CommitOutcome::Failed(_) => Err(WarmPoolError::Unavailable),
        }
    }
}
