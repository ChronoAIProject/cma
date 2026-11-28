//! Durable preparation and promotion of one exact native warm-pool plan.

use layer2_core::released_runtime::{CandidateRelease, ReleasedLaneRecord};
use mongodb::ClientSession;

use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

impl MongoNativeWarmPools {
    pub(super) async fn authorize_candidate_effect(
        &self,
        target: &PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        use crate::store::released_runtimes::MongoReleasedRuntimes;
        use layer2_core::released_runtime::{CandidateState, ReleasedLane};

        let lane = if target.machine {
            ReleasedLane::Machine
        } else {
            ReleasedLane::Agent
        };
        let lanes = MongoReleasedRuntimes::open(&self.database, &self.deployment);
        let current = lanes
            .load(lane)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let Some(current) = current.filter(|row| !row.frozen) else {
            return Ok(false);
        };
        let Some(candidate) = current.candidate.as_ref().filter(|candidate| {
            candidate.state == CandidateState::Preparing
                && candidate.image == target.plan.spec.image.uri
        }) else {
            return Ok(false);
        };
        let mut session = lanes
            .session()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        lanes
            .fence_current(&current, &mut session)
            .await
            .map_err(|_| WarmPoolError::Conflict)?;
        let mut filter = fence(target);
        filter.insert("preparation.observation_id", &candidate.observation_id);
        self.check_fenced_write(
            self.targets
                .update_one(
                    filter,
                    vec![doc! {"$set": {
                    "preparation.effect_until": {"$add": ["$$NOW", 60_000_i64]},
                    "preparation.pending_revision": target.revision,
                            }}],
                )
                .session(&mut session)
                .await,
        )
        .await?;
        session
            .commit_guarded()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        Ok(true)
    }
    pub(crate) async fn historical_candidate(
        &self,
        machine: bool,
        image: &str,
        session: &mut ClientSession,
    ) -> Result<bool, WarmPoolError> {
        let mut scope = retirement::release_scope(self, machine);
        scope.insert("plan.spec.image.uri", image);
        scope.insert(
            "$or",
            vec![
                doc! {"activated_at": {"$type": "date"}},
                doc! {"retiring": true},
                doc! {"retired": true},
            ],
        );
        Ok(self
            .targets
            .find_one(scope)
            .session(session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some())
    }
    pub(super) async fn current_retirement_image(
        &self,
        target: &PoolTarget,
        fallback: &str,
    ) -> Result<String, WarmPoolError> {
        let lane = if target.machine {
            layer2_core::released_runtime::ReleasedLane::Machine
        } else {
            layer2_core::released_runtime::ReleasedLane::Agent
        };
        if let Some(lanes) = &self.released_lanes {
            let record = lanes
                .load(lane)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            return record
                .map(|record| record.active.image)
                .ok_or(WarmPoolError::Unavailable);
        }
        Ok(fallback.into())
    }
    pub(crate) async fn verify_bootstrap(
        &self,
        machine: bool,
        image: &str,
    ) -> Result<(), WarmPoolError> {
        self.verify_bootstrap_in(machine, image, None).await
    }

    pub(super) async fn verify_bootstrap_in(
        &self,
        machine: bool,
        image: &str,
        mut session: Option<&mut ClientSession>,
    ) -> Result<(), WarmPoolError> {
        let mut scope = retirement::release_scope(self, machine);
        scope.insert("activated_at", doc! {"$type": "date"});
        let query = self
            .targets
            .find_one(scope.clone())
            .sort(retirement::release_order(machine));
        let Some(latest) = (match session.as_deref_mut() {
            Some(session) => query.session(session).await,
            None => query.await,
        })
        .map_err(|_| WarmPoolError::Unavailable)?
        else {
            return Ok(());
        };
        if latest.plan.spec.image.uri != image || latest.retiring || latest.retired {
            return Err(WarmPoolError::Conflict);
        }
        scope.extend(doc! {
            "activated_at": latest.activated_at,
            "plan.spec.image.uri": {"$ne": image},
        });
        let query = self.targets.find_one(scope);
        if (match session {
            Some(session) => query.session(session).await,
            None => query.await,
        })
        .map_err(|_| WarmPoolError::Unavailable)?
        .is_some()
        {
            return Err(WarmPoolError::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn prepare_candidate(
        &self,
        mut target: PoolTarget,
        candidate: &CandidateRelease,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let mut identity = self.scope();
        identity.extend(doc! {
            "plan.config_sha256": &target.plan.config_sha256,
            "retiring": {"$ne": true}, "retired": {"$ne": true},
            "activated_at": mongodb::bson::Bson::Null,
        });
        let marker = doc! {"observation_id": &candidate.observation_id};
        match self
            .targets
            .find_one(identity.clone())
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
        {
            Some(existing)
                if existing.plan == target.plan && existing.machine == target.machine =>
            {
                identity.extend(doc! {
                    "_id": &existing.id,
                    "revision": existing.revision,
                    "preparation": {"$exists": false},
                    "lease_until": {"$lte": DateTime::now()},
                });
                self.check_fenced_write(self.targets.update_one(identity, doc! {
                    "$set": {"preparation": marker, "target_replicas": i64::from(target.target_replicas),
                        "reconcile_after": DateTime::from_millis(0)},
                    "$inc": {"revision": 1_i64},
                }).session(session).await).await
            }
            Some(_) => Err(WarmPoolError::Conflict),
            None => {
                target.activated_at = None;
                let mut row = mongodb::bson::serialize_to_document(&target)
                    .map_err(|_| WarmPoolError::Unavailable)?;
                row.insert("preparation", marker);
                majority_collection::<Document>(&self.database, COLLECTION)
                    .insert_one(row)
                    .session(session)
                    .await
                    .map_err(|error| {
                        if super::super::sandbox_mongo::is_duplicate_key(&error) {
                            WarmPoolError::Conflict
                        } else {
                            WarmPoolError::Unavailable
                        }
                    })?;
                Ok(())
            }
        }
    }

    pub(crate) async fn clear_candidate(
        &self,
        machine: bool,
        candidate: Option<&CandidateRelease>,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let Some(candidate) = candidate else {
            return Ok(());
        };
        let mut scope = retirement::release_scope(self, machine);
        scope.extend(doc! {
            "plan.spec.image.uri": &candidate.image,
            "preparation.observation_id": &candidate.observation_id,
        });
        let Some(row) = self
            .targets
            .find_one(scope.clone())
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
        else {
            return Ok(());
        };
        scope.extend(doc! {"_id": &row.id, "revision": row.revision,
            "lease_until": {"$lte": DateTime::now()},
            "preparation.pending_revision": {"$exists": false},
            "$expr": {"$lte": [{"$ifNull": ["$preparation.effect_until", DateTime::from_millis(0)]}, "$$NOW"]},
        });
        self.check_fenced_write(
            self.targets
                .update_one(scope, doc! {"$unset": {"preparation": ""}})
                .session(session)
                .await,
        )
        .await
    }

    pub(crate) async fn prepared_candidate(
        &self,
        machine: bool,
        candidate: &CandidateRelease,
    ) -> Result<Option<PoolTarget>, WarmPoolError> {
        let mut scope = retirement::release_scope(self, machine);
        scope.extend(doc! {
            "plan.spec.image.uri": &candidate.image,
            "preparation.observation_id": &candidate.observation_id,
            "retired": {"$ne": true}, "retiring": {"$ne": true},
        });
        self.targets
            .find_one(scope)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .map(validate)
            .transpose()
    }

    pub(crate) async fn promote_candidate(
        &self,
        lane: &ReleasedLaneRecord,
        active: &PoolTarget,
        candidate: &PoolTarget,
        ready: &layer3_adapter::agent_sandbox::NativePoolStatus,
        promotion_at: DateTime,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        self.fence_activation(active.machine, session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let observed = lane.candidate.as_ref().ok_or(WarmPoolError::Conflict)?;
        if !ready.observed
            || candidate.binding.as_ref() != Some(&ready.binding)
            || active.target_replicas == 0
            || candidate.target_replicas != active.target_replicas
            || ready.desired_replicas != active.target_replicas
            || ready.ready_replicas < ready.desired_replicas
            || candidate.error.is_some()
            || candidate.applied_revision != candidate.revision
            || active.machine != candidate.machine
            || active.plan.spec.image.uri != lane.active.image
            || candidate.plan.spec.image.uri != observed.image
        {
            return Err(WarmPoolError::Conflict);
        }
        let mut scope = retirement::release_scope(self, active.machine);
        scope.insert("activated_at", doc! {"$type": "date"});
        let latest = self
            .targets
            .find_one(scope.clone())
            .sort(retirement::release_order(active.machine))
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .ok_or(WarmPoolError::Conflict)?;
        if latest.plan.spec.image.uri != lane.active.image
            || latest.retiring
            || latest.retired
            || (active.machine && latest.id != active.id)
            || latest.activated_at.is_none_or(|at| at >= promotion_at)
        {
            return Err(WarmPoolError::Conflict);
        }
        scope.insert("$or", vec![
            doc! {"plan.spec.image.uri": &observed.image},
            doc! {"activated_at": latest.activated_at, "plan.spec.image.uri": {"$ne": &lane.active.image}},
        ]);
        if self
            .targets
            .find_one(scope)
            .session(&mut *session)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some()
        {
            return Err(WarmPoolError::Conflict);
        }
        let mut active_filter = self.scope();
        active_filter.extend(doc! {
            "_id": &active.id, "revision": active.revision,
            "target_replicas": i64::from(active.target_replicas),
            "retiring": {"$ne": true}, "retired": {"$ne": true},
            "plan.spec.image.uri": &lane.active.image,
        });
        // A real write to the active target excludes a racing operator resize.
        self.check_fenced_write(
            self.targets
                .update_one(
                    active_filter,
                    doc! {
                        "$inc": {"promotion_fence": 1_i64},
                    },
                )
                .session(&mut *session)
                .await,
        )
        .await?;
        let mut filter = self.scope();
        let binding = mongodb::bson::serialize_to_bson(&candidate.binding)
            .map_err(|_| WarmPoolError::Unavailable)?;
        filter.extend(doc! {
            "_id": &candidate.id, "revision": candidate.revision,
            "target_replicas": i64::from(candidate.target_replicas),
            "applied_revision": candidate.applied_revision,
            "binding": binding, "error": mongodb::bson::Bson::Null,
            "preparation.observation_id": &observed.observation_id,
            "preparation.pending_revision": {"$exists": false},
            "activated_at": mongodb::bson::Bson::Null,
            "retired": {"$ne": true}, "retiring": {"$ne": true},
        });
        let mut fields = doc! {"activated_at": promotion_at};
        if candidate.machine {
            fields.insert("machine_plan_activated_at", promotion_at);
        }
        if let Some(choice) = active.operator_replicas {
            fields.insert(
                "operator_replicas",
                mongodb::bson::serialize_to_bson(&choice)
                    .map_err(|_| WarmPoolError::Unavailable)?,
            );
        }
        self.check_fenced_write(
            self.targets
                .update_one(
                    filter,
                    doc! {
                        "$set": fields, "$unset": {"preparation": ""},
                    },
                )
                .session(session)
                .await,
        )
        .await
    }

    pub(crate) async fn resize_candidate(
        &self,
        observation: &CandidateRelease,
        candidate: &PoolTarget,
        active: &PoolTarget,
        session: &mut ClientSession,
    ) -> Result<(), WarmPoolError> {
        let mut active_filter = self.scope();
        active_filter.extend(doc! {
            "_id": &active.id, "revision": active.revision,
            "target_replicas": i64::from(active.target_replicas),
            "retiring": {"$ne": true}, "retired": {"$ne": true},
        });
        self.check_fenced_write(
            self.targets
                .update_one(
                    active_filter,
                    doc! {
                        "$inc": {"promotion_fence": 1_i64},
                    },
                )
                .session(&mut *session)
                .await,
        )
        .await?;
        let mut filter = self.scope();
        filter.extend(doc! {
            "_id": &candidate.id, "revision": candidate.revision,
            "preparation.observation_id": &observation.observation_id,
            "lease_until": {"$lte": DateTime::now()},
            "retiring": {"$ne": true}, "retired": {"$ne": true},
        });
        self.check_fenced_write(
            self.targets
                .update_one(
                    filter,
                    doc! {
                        "$set": {"target_replicas": i64::from(active.target_replicas),
                            "reconcile_after": DateTime::from_millis(0)},
                        "$inc": {"revision": 1_i64},
                    },
                )
                .session(session)
                .await,
        )
        .await
    }
}
