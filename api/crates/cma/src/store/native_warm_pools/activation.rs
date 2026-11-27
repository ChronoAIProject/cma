//! Order fixed-pin activation and runtime publication on one durable lane fence.
use super::*;
use crate::store::released_runtimes::{BootWriter, MongoReleasedRuntimes};
use crate::store::{
    commit_transaction, is_confirmed_body_retry, start_transaction,
    transaction_terminal::GuardedTerminal as _, CommitOutcome,
};
use layer2_core::released_runtime::{ReleasedLane, ReleasedLaneRecord};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::ClientSession;
use std::time::Duration;

const ATTEMPTS: usize = 5;

impl MongoNativeWarmPools {
    fn cutover_authority(&self, epoch: uuid::Uuid, writer: &BootWriter) -> Document {
        // The rollout controller writes this authorization only after excluding
        // predecessor writers. The application never creates or enables it.
        doc! {
            "_id": format!("runtime_pointer_cutover:{}", self.deployment),
            "deployment": &self.deployment,
            "document_schema_version": crate::store::schema::DOCUMENT_SCHEMA_VERSION,
            "epoch": epoch.to_string(),
            "build_digest": &writer.build_digest,
            "state": "fenced",
        }
    }

    pub(crate) async fn verify_cutover(
        &self,
        epoch: uuid::Uuid,
        writer: &BootWriter,
    ) -> Result<(), WarmPoolError> {
        let settings =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        if settings
            .find_one(self.cutover_authority(epoch, writer))
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some()
        {
            Ok(())
        } else {
            Err(WarmPoolError::Unavailable)
        }
    }

    async fn fence_cutover(
        &self,
        epoch: uuid::Uuid,
        writer: &BootWriter,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let settings =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        // Withdrawal and publication must contend on the same record. A
        // stale approval cannot be used after the controller closes it.
        let result = settings
            .update_one(
                self.cutover_authority(epoch, writer),
                doc! {"$inc":{"decision_fence":1_i64}},
            )
            .session(session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(WarmPoolError::Unavailable)
        }
    }

    pub(super) async fn fence_activation(
        &self,
        machine: bool,
        session: &mut ClientSession,
    ) -> mongodb::error::Result<()> {
        if machine {
            return retirement::machine_selection::selection_fence(self, session).await;
        }
        // Fixed-pin writers may lack the optional release collection grant.
        // Settings are already required by both compatible writer modes.
        let coordination =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        coordination
            .update_one(
                doc! {"_id":format!("agent_pool_activation:{}", self.deployment),
                "deployment": &self.deployment,
                "document_schema_version": crate::store::schema::DOCUMENT_SCHEMA_VERSION},
                doc! {"$inc":{"decision_fence":1_i64}},
            )
            .upsert(true)
            .session(session)
            .await?;
        Ok(())
    }

    pub(super) async fn ordered_activation_session(
        &self,
        machine: bool,
    ) -> Result<ClientSession, WarmPoolError> {
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1 << attempt)).await;
            }
            let mut session = self
                .database
                .client()
                .start_session()
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            match self.fence_activation(machine, &mut session).await {
                Ok(()) => return Ok(session),
                Err(error) => {
                    if let Err(abort) = session.abort_guarded().await {
                        tracing::warn!(%abort, "runtime activation fence abort was not acknowledged");
                    }
                    if !is_confirmed_body_retry(&error) {
                        return Err(WarmPoolError::Unavailable);
                    }
                }
            }
        }
        Err(WarmPoolError::Conflict)
    }

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

    pub(super) async fn activate_agent(&self, candidate: &PoolTarget) -> Result<(), WarmPoolError> {
        let mut session = self.ordered_activation_session(false).await?;
        let result = self.activate_agent_in(candidate, &mut session).await;
        if let Err(error) = result {
            if let Err(abort) = session.abort_guarded().await {
                tracing::warn!(%abort, "Agent runtime activation abort was not acknowledged");
            }
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) => Err(WarmPoolError::Conflict),
            CommitOutcome::Unknown(error) => {
                tracing::warn!(%error, "Agent runtime activation commit is uncertain; checking durable target");
                if self.find(&candidate.id).await?.is_some_and(|row| {
                    row.plan == candidate.plan
                        && !row.retiring
                        && !row.retired
                        && row.activated_at.is_some()
                }) {
                    Ok(())
                } else {
                    Err(WarmPoolError::Unavailable)
                }
            }
            CommitOutcome::Failed(error) => {
                tracing::warn!(%error, "Agent runtime activation commit failed");
                Err(WarmPoolError::Unavailable)
            }
        }
    }

    async fn activate_agent_in(
        &self,
        candidate: &PoolTarget,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let image = &candidate.plan.spec.image.uri;
        self.verify_activation_floor(false, image, session).await?;
        let mut filter = self.scope();
        filter.extend(doc! {"_id": &candidate.id, "retiring":{"$ne":true}, "retired":{"$ne":true}});
        let row = self
            .targets
            .find_one(filter.clone())
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .ok_or(WarmPoolError::Conflict)?;
        if row.plan != candidate.plan || row.machine {
            return Err(WarmPoolError::Conflict);
        }
        if row.activated_at.is_some() {
            return Ok(());
        }
        let mut historical = retirement::release_scope(self, false);
        historical.extend(doc! {"plan.spec.image.uri": image, "activated_at":{"$type":"date"}});
        let previous = self
            .targets
            .find_one(historical)
            .sort(doc! {"activated_at":1})
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .and_then(|row| row.activated_at);
        // A resized or reconfigured plan keeps its image's first release order.
        let update = match previous {
            Some(at) => doc! {"$set":{"activated_at":at}},
            None => doc! {"$currentDate":{"activated_at":true}},
        };
        filter.insert("activated_at", mongodb::bson::Bson::Null);
        self.check_fenced_write(
            self.targets
                .update_one(filter, update)
                .session(session)
                .await,
        )
        .await
    }

    pub(crate) async fn publish_bootstrap(
        &self,
        target: &PoolTarget,
        lanes: &MongoReleasedRuntimes,
        writer: &BootWriter,
        record: &ReleasedLaneRecord,
        epoch: uuid::Uuid,
    ) -> Result<(), WarmPoolError> {
        if target.machine != (record.lane == ReleasedLane::Machine) {
            return Err(WarmPoolError::Conflict);
        }
        let mut session = self.ordered_activation_session(target.machine).await?;
        let result = self
            .publish_bootstrap_in(target, lanes, writer, record, epoch, &mut session)
            .await;
        if let Err(error) = result {
            if let Err(abort) = session.abort_guarded().await {
                tracing::warn!(%abort, "runtime pointer publication abort was not acknowledged");
            }
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) => Err(WarmPoolError::Conflict),
            CommitOutcome::Unknown(error) => {
                tracing::warn!(%error, "runtime pointer publication outcome is uncertain; checking durable lane");
                match lanes.load(record.lane).await {
                    Ok(Some(current)) if current == *record => Ok(()),
                    Ok(Some(_)) => Err(WarmPoolError::Conflict),
                    _ => Err(WarmPoolError::Unavailable),
                }
            }
            CommitOutcome::Failed(error) => {
                tracing::warn!(%error, "runtime pointer publication commit failed");
                Err(WarmPoolError::Unavailable)
            }
        }
    }

    async fn publish_bootstrap_in(
        &self,
        target: &PoolTarget,
        lanes: &MongoReleasedRuntimes,
        writer: &BootWriter,
        record: &ReleasedLaneRecord,
        epoch: uuid::Uuid,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        self.fence_cutover(epoch, writer, session).await?;
        self.verify_bootstrap_in(target.machine, &record.active.image, Some(&mut *session))
            .await?;
        lanes
            .protect_boot_effect_in(writer, Some(&mut *session))
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if target.machine {
            retirement::machine_selection::select_for_publication(self, target, session).await?;
        } else {
            self.activate_agent_in(target, session).await?;
        }
        self.verify_bootstrap_in(target.machine, &record.active.image, Some(&mut *session))
            .await?;
        let mut filter = self.scope();
        filter.extend(
            doc! {"_id": &target.id, "plan.spec.image.uri": &record.active.image,
            "activated_at": {"$type":"date"}, "retiring":{"$ne":true}, "retired":{"$ne":true}},
        );
        self.check_fenced_write(
            self.targets
                .update_one(filter, doc! {"$set":{"durable_release_authority":true}})
                .session(&mut *session)
                .await,
        )
        .await?;
        lanes
            .insert_bootstrap(record, Some(session))
            .await
            .map_err(|error| match error {
                SandboxStoreError::Conflict => WarmPoolError::Conflict,
                _ => WarmPoolError::Unavailable,
            })
    }
}
