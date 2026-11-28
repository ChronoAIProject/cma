//! Order prepare-only work and selection on the fixed lane's activation fence.
use super::*;
use crate::store::{commit_transaction, transaction_terminal::GuardedTerminal as _, CommitOutcome};
use crate::warm_pools::observe_fixed_spare;
use layer3_adapter::agent_sandbox::NativePoolControl;
use mongodb::{bson::Bson, ClientSession};

impl MongoNativeWarmPools {
    pub(super) async fn record_fixed_preparation(
        &self,
        active: &PoolTarget,
        mut target: PoolTarget,
    ) -> Result<(), WarmPoolError> {
        let mut session = self.ordered_activation_session(active.machine).await?;
        let result = self
            .record_fixed_preparation_in(active, &mut target, &mut session)
            .await;
        complete(&mut session, result).await
    }

    async fn record_fixed_preparation_in(
        &self,
        active: &PoolTarget,
        candidate: &mut PoolTarget,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        self.verify_fixed_incumbent(active.machine, &active.id, active.revision, session)
            .await?;
        if candidate.machine != active.machine || candidate.target_replicas == 0 {
            return Err(WarmPoolError::Conflict);
        }
        candidate.operator_replicas = active.operator_replicas;
        let rows = majority_collection::<Document>(&self.database, COLLECTION);
        let mut scope = retirement::release_scope(self, active.machine);
        scope.insert("preparation.fixed_incumbent", doc! {"$type": "string"});
        let marker =
            doc! {"fixed_incumbent": &active.id, "fixed_incumbent_revision": active.revision};
        if let Some(pending) = rows
            .find_one(scope)
            .session(&mut *session)
            .await
            .map_err(unavailable)?
        {
            if pending.get_str("_id").is_ok_and(|id| id == candidate.id)
                && pending
                    .get_document("preparation")
                    .is_ok_and(|preparation| preparation == &marker)
            {
                return Ok(());
            }
            return Err(WarmPoolError::Conflict);
        }
        let mut row = mongodb::bson::serialize_to_document(candidate).map_err(unavailable)?;
        row.insert("preparation", marker);
        rows.insert_one(row)
            .session(session)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    pub(super) async fn confirm_fixed_preparation(
        &self,
        target: &PoolTarget,
        active: &PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        let mut filter = fence(target);
        filter.extend(doc! {
            "preparation.fixed_incumbent": &active.id,
            "preparation.fixed_incumbent_revision": active.revision,
            "retiring": {"$ne": true}, "activated_at": Bson::Null,
        });
        Ok(self
            .targets
            .find_one(filter)
            .await
            .map_err(unavailable)?
            .is_some())
    }

    pub(super) async fn select_fixed_preparations(
        &self,
        targets: &[PoolTarget],
        provider: &dyn NativePoolControl,
        target_fencing: bool,
    ) -> Result<(), WarmPoolError> {
        let Some(first) = targets.first() else {
            return Ok(());
        };
        let mut session = self.ordered_activation_session(first.machine).await?;
        let result = async {
            if targets
                .iter()
                .skip(1)
                .any(|target| target.machine != first.machine)
            {
                self.fence_activation(!first.machine, &mut session)
                    .await
                    .map_err(unavailable)?;
            }
            for target in targets {
                self.select_fixed_preparation_in(target, provider, target_fencing, &mut session)
                    .await?;
            }
            Ok(())
        }
        .await;
        complete(&mut session, result).await
    }

    async fn select_fixed_preparation_in(
        &self,
        target: &PoolTarget,
        provider: &dyn NativePoolControl,
        target_fencing: bool,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        self.verify_activation_floor(target.machine, &target.plan.spec.image.uri, session)
            .await?;
        let mut filter = retirement::release_scope(self, target.machine);
        filter.extend(doc! {
            "_id": &target.id, "revision": target.revision,
            "applied_revision": target.revision, "target_replicas": i64::from(target.target_replicas),
            "binding": mongodb::bson::serialize_to_bson(&target.binding).map_err(unavailable)?,
            "error": Bson::Null, "lease": Bson::Null,
            "retiring": {"$ne":true}, "retired": {"$ne":true},
        });
        let rows = majority_collection::<Document>(&self.database, COLLECTION);
        let row = rows
            .find_one(filter.clone())
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(WarmPoolError::Conflict)?;
        observe_fixed_spare(provider, target, target_fencing).await?;
        if row.get_datetime("activated_at").is_ok() {
            return self
                .verify_fixed_incumbent(target.machine, &target.id, target.revision, session)
                .await;
        }
        let marker = row.get_document("preparation").map_err(unavailable)?;
        let incumbent = marker.get_str("fixed_incumbent").map_err(unavailable)?;
        let revision = marker
            .get_i64("fixed_incumbent_revision")
            .map_err(unavailable)?;
        self.verify_fixed_incumbent(target.machine, incumbent, revision, session)
            .await?;
        filter.extend(doc! {"preparation": marker, "activated_at": Bson::Null});
        let mut dates = doc! {"activated_at": true};
        if target.machine {
            dates.insert("machine_plan_activated_at", true);
        }
        self.check_fenced_write(
            rows.update_one(
                filter,
                doc! {"$currentDate": dates, "$unset": {"preparation": ""}},
            )
            .session(session)
            .await,
        )
        .await
    }

    async fn verify_fixed_incumbent(
        &self,
        machine: bool,
        id: &str,
        revision: i64,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let mut scope = retirement::release_scope(self, machine);
        scope.insert("activated_at", doc! {"$type": "date"});
        let last = self
            .targets
            .find_one(scope)
            .sort(retirement::release_order(machine))
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let last = last
            .filter(|row| {
                row.id == id
                    && row.revision == revision
                    && !row.retiring
                    && !row.retired
                    && row.target_replicas > 0
            })
            .ok_or(WarmPoolError::Conflict)?;
        let mut tied = retirement::release_scope(self, machine);
        tied.extend(doc! {"activated_at": last.activated_at, "_id": {"$ne": id}});
        if machine {
            tied.insert("machine_plan_activated_at", last.machine_plan_activated_at);
        } else {
            tied.insert(
                "plan.spec.image.uri",
                doc! {"$ne": &last.plan.spec.image.uri},
            );
        }
        if self
            .targets
            .find_one(tied)
            .session(session)
            .await
            .map_err(unavailable)?
            .is_some()
        {
            return Err(WarmPoolError::Conflict);
        }
        Ok(())
    }
}

async fn complete(
    session: &mut ClientSession,
    result: Result<(), WarmPoolError>,
) -> Result<(), WarmPoolError> {
    match result {
        Ok(()) => match commit_transaction(session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) => Err(WarmPoolError::Conflict),
            _ => Err(WarmPoolError::Unavailable),
        },
        Err(error) => {
            let _ = session.abort_guarded().await;
            Err(error)
        }
    }
}
fn unavailable(_: impl std::fmt::Display) -> WarmPoolError {
    WarmPoolError::Unavailable
}
