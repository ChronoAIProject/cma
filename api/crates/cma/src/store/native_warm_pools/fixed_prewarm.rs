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
        mut candidate: Option<PoolTarget>,
        generation: i64,
    ) -> Result<(), WarmPoolError> {
        let mut session = self.ordered_activation_session(active.machine).await?;
        let result = self
            .record_fixed_preparation_in(active, candidate.as_mut(), generation, &mut session)
            .await;
        complete(&mut session, result).await
    }

    async fn record_fixed_preparation_in(
        &self,
        active: &PoolTarget,
        candidate: Option<&mut PoolTarget>,
        generation: i64,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        self.verify_fixed_incumbent(active.machine, &active.id, active.revision, session)
            .await?;
        let target_id = candidate.as_ref().map(|row| row.id.as_str());
        let settings =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        let intent = self.fixed_intent_key(active.machine);
        if let Some(previous) = settings
            .find_one(intent.clone())
            .session(&mut *session)
            .await
            .map_err(unavailable)?
        {
            let current = previous.get_i64("generation").map_err(unavailable)?;
            if generation < current {
                return Ok(());
            }
            if generation == current && previous.get_str("candidate").ok() != target_id {
                // A zeroed incumbent withdraws its own candidate. Explicit
                // withdrawal or another digest uses a new generation.
                if active.target_replicas != 0 {
                    return Err(WarmPoolError::Conflict);
                }
            }
        }
        settings
            .update_one(
                intent,
                doc! {"$set": {
                    "generation": generation,
                    "candidate": target_id.map_or(Bson::Null, Into::into),
                }},
            )
            .upsert(true)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;

        let rows = majority_collection::<Document>(&self.database, COLLECTION);
        let mut scope = retirement::release_scope(self, active.machine);
        scope.extend(doc! {
            "preparation.fixed_incumbent": {"$type": "string"},
            "preparation.withdrawn": {"$ne": true},
        });
        if let Some(target_id) = target_id {
            scope.insert("_id", doc! {"$ne": target_id});
        }
        rows.update_many(scope, doc! {"$set": {"preparation.withdrawn": true}})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let Some(candidate) = candidate else {
            return Ok(());
        };
        if candidate.machine != active.machine || candidate.target_replicas == 0 {
            return Err(WarmPoolError::Conflict);
        }
        candidate.operator_replicas = active.operator_replicas;
        let marker = doc! {
            "fixed_incumbent": &active.id, "fixed_incumbent_revision": active.revision,
            "generation": generation,
        };
        let mut exact = retirement::release_scope(self, active.machine);
        exact.insert("_id", &candidate.id);
        if let Some(current) = rows
            .find_one(exact.clone())
            .session(&mut *session)
            .await
            .map_err(unavailable)?
        {
            let current_marker = current.get_document("preparation").map_err(unavailable)?;
            if current_marker.get_str("fixed_incumbent").ok() != Some(active.id.as_str())
                || current_marker.get_bool("withdrawn").ok() == Some(true)
            {
                return Err(WarmPoolError::Conflict);
            }
            if current_marker == &marker {
                return Ok(());
            }
            // A resize or new generation can reuse settled capacity. Keep
            // the old effect fence until its provider operation completes.
            if current_marker.contains_key("pending_revision")
                || current
                    .get_datetime("lease_until")
                    .is_ok_and(|lease| *lease > DateTime::now())
            {
                return Ok(());
            }
            exact.extend(doc! {
                "revision": current.get_i64("revision").map_err(unavailable)?,
                "lease": Bson::Null, "preparation": current_marker,
            });
            return self.check_fenced_write(rows.update_one(exact, doc! {
                "$set": {
                    "preparation": marker, "target_replicas": i64::from(candidate.target_replicas),
                    "reconcile_after": DateTime::from_millis(0),
                    "operator_replicas": mongodb::bson::serialize_to_bson(&candidate.operator_replicas).map_err(unavailable)?,
                },
                "$inc": {"revision": 1_i64},
            }).session(session).await).await;
        }
        let mut row = mongodb::bson::serialize_to_document(candidate).map_err(unavailable)?;
        row.insert("preparation", marker);
        rows.insert_one(row)
            .session(session)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    fn fixed_intent_key(&self, machine: bool) -> Document {
        doc! {
            "_id": format!("fixed_pool_prewarm:{}:{machine}", self.deployment),
            "deployment": &self.deployment,
            "document_schema_version": crate::store::schema::DOCUMENT_SCHEMA_VERSION,
        }
    }

    pub(super) async fn read_fixed_preparation(
        &self,
        target: &PoolTarget,
    ) -> Result<Option<bool>, WarmPoolError> {
        let mut filter = fence(target);
        filter.insert("preparation.fixed_incumbent", doc! {"$type":"string"});
        let rows = majority_collection::<Document>(&self.database, COLLECTION);
        Ok(rows
            .find_one(filter)
            .await
            .map_err(unavailable)?
            .map(|row| {
                row.get_document("preparation")
                    .is_ok_and(|marker| marker.get_bool("withdrawn").ok() == Some(true))
            }))
    }

    pub(super) async fn authorize_fixed_effect(
        &self,
        target: &PoolTarget,
        active: &PoolTarget,
        generation: i64,
    ) -> Result<bool, WarmPoolError> {
        let mut session = self.ordered_activation_session(active.machine).await?;
        let result = async {
            self.verify_fixed_incumbent(active.machine, &active.id, active.revision, &mut session)
                .await?;
            let mut intent = self.fixed_intent_key(active.machine);
            intent.extend(doc! {"generation": generation, "candidate": &target.id});
            let settings =
                majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
            if settings
                .find_one(intent)
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .is_none()
            {
                return Ok(false);
            }
            let mut filter = fence(target);
            filter.extend(doc! {
                "preparation.fixed_incumbent": &active.id, "preparation.withdrawn": {"$ne": true},
                "retiring": {"$ne": true}, "activated_at": Bson::Null,
            });
            // Set the pending marker before any provider request can dispatch.
            let update = self
                .targets
                .update_one(
                    filter,
                    doc! {"$set": {
                        "preparation.pending_revision": target.revision,
                    }},
                )
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            Ok(update.matched_count == 1)
        }
        .await;
        match result {
            Ok(written) => {
                complete(&mut session, Ok(())).await?;
                Ok(written)
            }
            Err(error) => {
                complete(&mut session, Err(error)).await?;
                Ok(false)
            }
        }
    }

    pub(super) async fn clear_withdrawn_fixed_preparation(
        &self,
        target: &PoolTarget,
    ) -> Result<(), WarmPoolError> {
        let mut filter = fence(target);
        filter.extend(doc! { "preparation.withdrawn": true, "preparation.pending_revision": {"$exists": false} });
        self.check_fenced_write(
            self.targets
                .update_one(
                    filter,
                    doc! {
                        "$set": {"lease": Bson::Null, "lease_until": DateTime::from_millis(0),
                            "reconcile_after": DateTime::from_millis(0)},
                        "$unset": {"preparation": ""},
                    },
                )
                .await,
        )
        .await
    }

    pub(super) async fn select_fixed_preparations(
        &self,
        targets: &[PoolTarget],
        provider: &dyn NativePoolControl,
        target_fencing: bool,
        generation: i64,
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
                self.select_fixed_preparation_in(
                    target,
                    provider,
                    target_fencing,
                    generation,
                    &mut session,
                )
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
        generation: i64,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        self.verify_activation_floor(target.machine, &target.plan.spec.image.uri, session)
            .await?;
        if target.activated_at.is_some() {
            // Restarts use normal readiness only after confirming that this
            // exact plan remains selected under the release fence.
            return self
                .verify_fixed_incumbent(target.machine, &target.id, target.revision, session)
                .await;
        }
        let settings =
            majority_collection::<Document>(&self.database, crate::store::settings::SETTINGS);
        let mut intent = self.fixed_intent_key(target.machine);
        intent.extend(doc! {"generation": generation, "candidate": &target.id});
        if settings
            .find_one(intent)
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .is_none()
        {
            return Err(WarmPoolError::Conflict);
        }
        let mut filter = retirement::release_scope(self, target.machine);
        filter.extend(doc! {
            "_id": &target.id, "revision": target.revision, "activated_at": Bson::Null,
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
        let marker = row.get_document("preparation").map_err(unavailable)?;
        if marker.get_i64("generation").ok() != Some(generation)
            || marker.get_bool("withdrawn").ok() == Some(true)
            || marker.contains_key("pending_revision")
        {
            return Err(WarmPoolError::Conflict);
        }
        let incumbent = marker.get_str("fixed_incumbent").map_err(unavailable)?;
        let revision = marker
            .get_i64("fixed_incumbent_revision")
            .map_err(unavailable)?;
        self.verify_fixed_incumbent(target.machine, incumbent, revision, session)
            .await?;
        // A concurrent resize changes this same incumbent row. Contend on it
        // before selecting a candidate prepared for its previous target.
        let mut incumbent_fence = retirement::release_scope(self, target.machine);
        incumbent_fence.extend(doc! {
            "_id": incumbent, "revision": revision, "target_replicas": {"$gt": 0_i64},
            "retiring": {"$ne": true}, "retired": {"$ne": true},
        });
        self.check_fenced_write(
            self.targets
                .update_one(
                    incumbent_fence,
                    doc! {"$inc": {"fixed_prewarm_selection_fence": 1_i64}},
                )
                .session(&mut *session)
                .await,
        )
        .await?;
        observe_fixed_spare(provider, target, target_fencing).await?;
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
                    && (!machine || row.machine_plan_activated_at.is_some())
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
