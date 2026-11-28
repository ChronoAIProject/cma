//! Majority-acknowledged pool plans, original UIDs and desired capacity.
use async_trait::async_trait;
use futures_util::TryStreamExt;
use layer1_public::rest::admin::warm_pools::{ResizeWarmPoolBody, WarmPoolError};
use layer2_core::StoreError;
use layer3_adapter::agent_sandbox::{NativePoolBinding, NativePoolConfiguration};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{IndexOptions, ReturnDocument},
    Collection, Database, IndexModel,
};

use super::durability::majority_collection;
use crate::warm_pools::{
    OperatorReplicas, PoolFailure, PoolTarget, PoolTargetStore, RetirementScan,
};

pub(super) const COLLECTION: &str = "native_warm_pool_targets";
mod activation;
mod create_cutover;
mod failure;
mod fixed_prewarm;
mod released;
mod retirement;
#[cfg(test)]
pub(crate) use retirement::eligibility_tests::quarantined_retirement_fixture;
mod schema;

const MAX_TARGETS: i64 = 1000;

pub(crate) struct MongoNativeWarmPools {
    targets: Collection<PoolTarget>,
    deployment: String,
    database: Database,
    pub(super) released_lanes: Option<super::released_runtimes::MongoReleasedRuntimes>,
}

impl MongoNativeWarmPools {
    pub async fn open(database: &Database, deployment: &str) -> Result<Self, StoreError> {
        schema::initialize(database).await?;
        let targets = majority_collection(database, COLLECTION);
        let unique = IndexModel::builder()
            .keys(doc! {"plan.deployment":1,"plan.config_sha256":1})
            .options(IndexOptions::builder().unique(true).build())
            .build();
        targets
            .create_index(unique)
            .await
            .map_err(|_| StoreError::Unavailable("create native pool identity index".into()))?;
        Ok(Self {
            targets,
            deployment: deployment.into(),
            database: database.clone(),
            released_lanes: None,
        })
    }

    fn scope(&self) -> Document {
        doc! {"plan.deployment":&self.deployment, "document_schema_version":{"$in":[1_i64,2_i64]}}
    }

    async fn check_fenced_write(
        &self,
        result: mongodb::error::Result<mongodb::results::UpdateResult>,
    ) -> Result<(), WarmPoolError> {
        let result = result.map_err(|_| WarmPoolError::Unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(WarmPoolError::Conflict)
        }
    }
}

fn fence(target: &PoolTarget) -> Document {
    doc! {"_id":&target.id,"revision":target.revision,"lease":target.lease.clone(),
    "lease_until":{"$gt":DateTime::now()},"plan.deployment":&target.plan.deployment,
    "document_schema_version":target.document_schema_version,"retired":{"$ne":true}}
}

fn validate(target: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
    if target.document_schema_version != if target.machine { 2 } else { 1 }
        || target.revision < 1
        || target.applied_revision < 0
        || target.applied_revision > target.revision
        || (target.machine_plan_activated_at.is_some()
            && (!target.machine || target.activated_at.is_none()))
        || (target.retired && !target.retiring)
        || (target.retiring && target.target_replicas != 0)
        || target.target_replicas > 1000
        || target
            .operator_replicas
            .is_some_and(|choice| choice.replicas > 1000)
        || target.plan.spec.validate().is_err()
        || target.binding.as_ref().is_some_and(|binding| {
            binding.spec != target.plan.spec
                || binding.pod_policy != target.plan.pod_policy
                || binding.config_sha256 != target.plan.config_sha256
        })
    {
        return Err(WarmPoolError::Unavailable);
    }
    Ok(target)
}

#[async_trait]
impl PoolTargetStore for MongoNativeWarmPools {
    async fn begin_candidate_effect(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        self.authorize_candidate_effect(target).await
    }

    async fn candidate_pending(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        let mut filter = fence(target);
        filter.insert("preparation.pending_revision", doc! {"$type": "long"});
        Ok(self
            .targets
            .find_one(filter)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some())
    }

    async fn confirm_candidate_barrier(&self, target: &PoolTarget) -> Result<(), WarmPoolError> {
        let mut filter = fence(target);
        filter.insert("preparation.pending_revision", doc! {"$type": "long"});
        self.check_fenced_write(self.targets.update_one(filter, doc! {
            "$set": {"retiring": true, "target_replicas": 0_i64, "lease": mongodb::bson::Bson::Null,
                "lease_until": DateTime::from_millis(0), "reconcile_after": DateTime::from_millis(0)},
            "$inc": {"revision": 1_i64}, "$unset": {"preparation.pending_revision": ""},
        }).await).await
    }
    async fn candidate_protected(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        let mut filter = self.scope();
        filter.extend(doc! {"_id": &target.id, "preparation": {"$exists": true}});
        Ok(self
            .targets
            .find_one(filter)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .is_some())
    }
    async fn activate(&self, id: &str) -> Result<(), WarmPoolError> {
        retirement::activate(self, id).await
    }
    async fn default_pin_conflicts(
        &self,
        plan: &NativePoolConfiguration,
        machine: bool,
    ) -> Result<bool, WarmPoolError> {
        let mut scope = self.scope();
        scope.insert("plan.config_sha256", &plan.config_sha256);
        let exact = self
            .targets
            .find_one(scope)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if let Some(exact) = exact.map(validate).transpose()? {
            if exact.machine != machine || exact.retiring || exact.retired {
                return Ok(true);
            }
            if machine && exact.activated_at.is_some() {
                let mut scope = retirement::release_scope(self, true);
                scope.insert("activated_at", doc! {"$type": "date"});
                let latest = self
                    .targets
                    .find_one(scope)
                    .sort(retirement::release_order(true))
                    .await
                    .map_err(|_| WarmPoolError::Unavailable)?;
                if let Some(row) = latest {
                    if row.machine_plan_activated_at.is_some()
                        && (row.id != exact.id
                            || retirement::machine_selection_ambiguous(self, &row).await?)
                    {
                        return Ok(true);
                    }
                }
            }
        }
        match self.verify_bootstrap(machine, &plan.spec.image.uri).await {
            Ok(()) => Ok(false),
            Err(WarmPoolError::Conflict) => Ok(true),
            Err(error) => Err(error),
        }
    }
    async fn retirement_homes(
        &self,
        target: &PoolTarget,
        image: &str,
    ) -> Result<Option<Vec<layer3_adapter::HomeVolumeAttachment>>, WarmPoolError> {
        let durable = self.current_retirement_image(target, image).await?;
        retirement::homes(self, target, &durable).await
    }
    async fn scan_retirement_drain(
        &self,
        target: &PoolTarget,
        image: &str,
    ) -> Result<RetirementScan, WarmPoolError> {
        let durable = self.current_retirement_image(target, image).await?;
        retirement::drain_homes(self, target, &durable).await
    }
    async fn defer_quarantined_retirement(&self, target: &PoolTarget) -> Result<(), WarmPoolError> {
        self.check_fenced_write(self.targets.update_one(fence(target), doc! {
            "$set": {
                "lease": mongodb::bson::Bson::Null,
                "lease_until": DateTime::from_millis(0),
                "reconcile_after": DateTime::from_millis(DateTime::now().timestamp_millis() + 15_000),
            }
        }).await).await
    }
    async fn begin_retirement(&self, target: &PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        retirement::begin(self, target).await
    }
    async fn remove_retired(&self, target: &PoolTarget) -> Result<(), WarmPoolError> {
        retirement::remove(self, target).await
    }
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError> {
        let rows: Vec<_> = self
            .targets
            .find({
                let mut scope = self.scope();
                scope.insert("retired", doc! {"$ne":true});
                scope
            })
            .sort(doc! {"_id":1})
            .limit(MAX_TARGETS + 1)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if rows.len() > MAX_TARGETS as usize {
            return Err(WarmPoolError::Unavailable);
        }
        rows.into_iter().map(validate).collect()
    }

    async fn find(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        let mut filter = self.scope();
        filter.insert("_id", id);
        self.targets
            .find_one(filter)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .map(validate)
            .transpose()
    }

    async fn insert(&self, target: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        self.insert_target(target, false).await
    }

    async fn insert_with_create_fence(
        &self,
        target: PoolTarget,
    ) -> Result<PoolTarget, WarmPoolError> {
        self.insert_target(target, true).await
    }

    async fn authorize_provider_create(&self, target: &PoolTarget) -> Result<(), WarmPoolError> {
        let mut filter = fence(target);
        filter.extend(doc! {"retiring": {"$ne": true}, "binding": mongodb::bson::Bson::Null});
        self.check_fenced_write(
            self.targets
                .update_one(
                    filter,
                    doc! {"$set": {"provider_create_never_dispatched": false}},
                )
                .await,
        )
        .await
    }

    async fn prepare_fixed_pool(
        &self,
        active: &PoolTarget,
        target: PoolTarget,
    ) -> Result<(), WarmPoolError> {
        self.record_fixed_preparation(active, target).await
    }

    async fn activate_fixed_pools(
        &self,
        targets: &[PoolTarget],
        provider: &dyn layer3_adapter::agent_sandbox::NativePoolControl,
        target_fencing: bool,
    ) -> Result<(), WarmPoolError> {
        self.select_fixed_preparations(targets, provider, target_fencing)
            .await
    }

    async fn fixed_pool_prepared_from(
        &self,
        target: &PoolTarget,
        active: &PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        self.confirm_fixed_preparation(target, active).await
    }

    async fn pool_creates_fenced(&self, epoch: uuid::Uuid) -> Result<bool, WarmPoolError> {
        self.create_cutover_is_current(epoch).await
    }
    async fn remove_unbound_retired(
        &self,
        target: &PoolTarget,
        epoch: uuid::Uuid,
    ) -> Result<(), WarmPoolError> {
        self.retire_after_create_cutover(target, epoch).await
    }
    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
        operator_choice: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        let mut filter = self.scope();
        filter.extend(doc! {"_id":id,"revision":body.expected_revision as i64,"retiring":{"$ne":true},"retired":{"$ne":true},"lease_until":{"$lte":DateTime::now()}});
        let mut fields = doc! {"target_replicas":i64::from(body.target_replicas),
        "error":mongodb::bson::Bson::Null,"reconciliation_failure":mongodb::bson::Bson::Null,"reconcile_after":DateTime::from_millis(0)};
        if operator_choice {
            // Same fenced write as the count, so the decision is never ahead of it.
            filter.insert("machine", true);
            fields.insert(
                "operator_replicas",
                doc! {"replicas":i64::from(body.target_replicas),"at":DateTime::now()},
            );
        }
        let update = doc! {"$set":fields,"$inc":{"revision":1_i64}};
        let updated = self
            .targets
            .find_one_and_update(filter, update)
            .return_document(ReturnDocument::After)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        match updated {
            Some(target) => validate(target),
            None => {
                if self.find(id).await?.is_none() {
                    Err(WarmPoolError::NotFound)
                } else {
                    Err(WarmPoolError::Conflict)
                }
            }
        }
    }

    async fn latest_machine_operator_replicas(
        &self,
    ) -> Result<Option<OperatorReplicas>, WarmPoolError> {
        // Retired tombstones keep the decision; the scope does not exclude them.
        let mut filter = self.scope();
        filter.extend(doc! {"machine":true,"operator_replicas.at":{"$type":"date"}});
        Ok(self
            .targets
            .find_one(filter)
            .sort(doc! {"operator_replicas.at":-1,"_id":1})
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .map(validate)
            .transpose()?
            .and_then(|target| target.operator_replicas))
    }

    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.claim_matching(None, true).await
    }

    async fn claim_target(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.claim_matching(Some(id), false).await
    }

    async fn claim_retirement(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.claim_matching(Some(id), true).await
    }

    async fn bind(
        &self,
        target: &PoolTarget,
        binding: &NativePoolBinding,
    ) -> Result<(), WarmPoolError> {
        if binding.spec != target.plan.spec
            || binding.pod_policy != target.plan.pod_policy
            || binding.config_sha256 != target.plan.config_sha256
        {
            return Err(WarmPoolError::Conflict);
        }
        let mut filter = fence(target);
        filter.insert("binding", mongodb::bson::Bson::Null);
        let value =
            mongodb::bson::serialize_to_bson(binding).map_err(|_| WarmPoolError::Unavailable)?;
        self.check_fenced_write(
            self.targets
                .update_one(filter, doc! {"$set":{"binding":value}})
                .await,
        )
        .await
    }

    async fn finish(&self, target: &PoolTarget, success: bool) -> Result<(), WarmPoolError> {
        let failure = (!success).then_some(PoolFailure::apply(
            "unavailable",
            "The native warm pool could not reconcile. Check provider availability and the recorded pool binding. Reconciliation retries automatically.",
        ));
        self.finish_attempt(target, failure).await
    }

    async fn finish_with_failure(
        &self,
        target: &PoolTarget,
        failure: PoolFailure,
    ) -> Result<(), WarmPoolError> {
        self.finish_attempt(target, Some(failure)).await
    }
}

impl MongoNativeWarmPools {
    async fn insert_target(
        &self,
        target: PoolTarget,
        fenced: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        let mut row = mongodb::bson::serialize_to_document(&target)
            .map_err(|_| WarmPoolError::Unavailable)?;
        if fenced && target.binding.is_none() {
            row.insert("provider_create_never_dispatched", true);
        }
        let targets = majority_collection::<Document>(&self.database, COLLECTION);
        match targets.insert_one(row).await {
            Ok(_) => Ok(target),
            Err(_) => {
                // Includes a lost acknowledgement: the key remains authoritative.
                if let Some(existing) = self.find(&target.id).await? {
                    return Ok(existing);
                }
                let mut filter = self.scope();
                filter.insert("plan.config_sha256", &target.plan.config_sha256);
                if self
                    .targets
                    .find_one(filter)
                    .await
                    .map_err(|_| WarmPoolError::Unavailable)?
                    .is_some()
                {
                    Err(WarmPoolError::Conflict)
                } else {
                    Err(WarmPoolError::Unavailable)
                }
            }
        }
    }

    async fn claim_matching(
        &self,
        id: Option<&str>,
        include_retiring: bool,
    ) -> Result<Option<PoolTarget>, WarmPoolError> {
        let now = DateTime::now();
        let mut filter = self.scope();
        filter.extend(
            doc! {"retired":{"$ne":true},"lease_until":{"$lte":now},"reconcile_after":{"$lte":now}},
        );
        if let Some(id) = id {
            filter.insert("_id", id);
        }
        if !include_retiring {
            filter.insert("retiring", doc! {"$ne":true});
        }
        self.targets.find_one_and_update(filter, doc! {"$set":{
            "lease":uuid::Uuid::new_v4().to_string(),"lease_until":DateTime::from_millis(now.timestamp_millis()+60_000)}})
            .sort(doc! {"reconcile_after":1,"_id":1}).return_document(ReturnDocument::After)
            .await.map_err(|_| WarmPoolError::Unavailable)?.map(validate).transpose()
    }
}
