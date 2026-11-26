//! Serialize fixed-pin activation with the first pointer publication for each lane.
use super::*;
use crate::store::released_runtimes::{BootWriter, MongoReleasedRuntimes};
use crate::store::{commit_transaction, is_confirmed_body_retry, CommitOutcome};
use layer2_core::released_runtime::ReleasedLaneRecord;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::ClientSession;

impl MongoNativeWarmPools {
    pub(super) async fn order_activation(
        &self,
        machine: bool,
    ) -> Result<ClientSession, WarmPoolError> {
        // Fixed-pin writers can lack access to the optional released-runtime collections.
        // The required settings collection already hosts deployment coordination records.
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
            let ordered = if machine {
                retirement::machine_selection::selection_fence(self, &mut session).await
            } else {
                let guards = majority_collection::<Document>(
                    &self.database,
                    crate::store::settings::SETTINGS,
                );
                guards
                    .update_one(
                        doc! {"_id":format!("agent_pool_activation:{}",self.deployment),
                        "deployment": &self.deployment, "document_schema_version":1_i64},
                        doc! {"$inc":{"decision_fence":1_i64}},
                    )
                    .upsert(true)
                    .session(&mut session)
                    .await
                    .map(|_| ())
            };
            match ordered {
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
        if target.machine {
            retirement::machine_selection::activate(self, &target).await?;
            let selected = self.find(id).await?.ok_or(WarmPoolError::Conflict)?;
            let mut filter = retirement::release_scope(self, true);
            filter.insert("activated_at", doc! {"$type":"date"});
            let latest = self
                .targets
                .find_one(filter)
                .sort(retirement::release_order(true))
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
                .ok_or(WarmPoolError::Conflict)?;
            if latest.id != selected.id
                || selected.retired
                || selected.retiring
                || retirement::machine_selection_ambiguous(self, &selected).await?
            {
                return Err(WarmPoolError::Conflict);
            }
            return self.verify_bootstrap(true, image).await;
        }
        if target.activated_at.is_some() {
            return self.verify_bootstrap(target.machine, image).await;
        }
        let mut session = self.order_activation(target.machine).await?;
        self.verify_activation_floor(target.machine, image, &mut session)
            .await?;
        let mut historical = retirement::release_scope(self, target.machine);
        historical.extend(doc! {"plan.spec.image.uri": image, "activated_at": {"$type": "date"}});
        let first = self
            .targets
            .find_one(historical)
            .sort(doc! {"activated_at": 1})
            .session(&mut session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let update = match first.and_then(|row| row.activated_at) {
            Some(at) => doc! {"$set": {"activated_at": at, "ordered_activation":true}},
            None => {
                doc! {"$currentDate": {"activated_at": true}, "$set":{"ordered_activation":true}}
            }
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
                }) => {}
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
        self.prepare_publication_index(target.machine, lanes, writer)
            .await?;
        let mut session = self.order_activation(target.machine).await?;
        self.verify_bootstrap_in(target.machine, &record.active.image, Some(&mut session))
            .await?;
        let mut filter = self.scope();
        filter.extend(doc! {"_id": &target.id, "plan.spec.image.uri": &record.active.image, "activated_at": {"$type": "date"},
            "retiring": {"$ne": true}, "retired": {"$ne": true}});
        let claimed = self
            .targets
            .update_one(
                filter,
                doc! {
                    "$set": {"durable_release_authority":true,"first_pointer_anchor":true},
                    "$unset": {"ordered_activation":""},
                },
            )
            .session(&mut session)
            .await;
        match claimed {
            Err(ref error) if crate::store::is_duplicate_key(error) => {
                return Err(WarmPoolError::Conflict)
            }
            result => self.check_fenced_write(result).await?,
        }
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
