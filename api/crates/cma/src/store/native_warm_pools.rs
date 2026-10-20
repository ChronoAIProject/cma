//! Majority-acknowledged pool plans, original UIDs and desired capacity.
use async_trait::async_trait;
use futures_util::TryStreamExt;
use layer1_public::rest::admin::warm_pools::{ResizeWarmPoolBody, WarmPoolError};
use layer2_core::StoreError;
use layer3_adapter::agent_sandbox::NativePoolBinding;
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{IndexOptions, ReturnDocument},
    Collection, Database, IndexModel,
};

use super::{durability::majority_collection, schema};
use crate::warm_pools::{PoolTarget, PoolTargetStore};

pub(super) const COLLECTION: &str = "native_warm_pool_targets";
mod retirement;

const MAX_TARGETS: i64 = 1000;

pub(crate) struct MongoNativeWarmPools {
    targets: Collection<PoolTarget>,
    deployment: String,
    database: Database,
}

impl MongoNativeWarmPools {
    pub async fn open(database: &Database, deployment: &str) -> Result<Self, StoreError> {
        schema::initialize_collections(database, &[COLLECTION]).await?;
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
        })
    }

    fn scope(&self) -> Document {
        doc! {"plan.deployment":&self.deployment, "document_schema_version":1_i64}
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
    "document_schema_version":1_i64,"retired":{"$ne":true}}
}

fn validate(target: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
    if target.document_schema_version != 1
        || target.revision < 1
        || target.applied_revision < 0
        || target.applied_revision > target.revision
        || (target.retired && !target.retiring)
        || (target.retiring && target.target_replicas != 0)
        || target.target_replicas > 1000
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
    async fn activate(&self, id: &str) -> Result<(), WarmPoolError> {
        retirement::activate(self, id).await
    }
    async fn retirement_homes(
        &self,
        target: &PoolTarget,
        image: &str,
    ) -> Result<Option<Vec<layer3_adapter::HomeVolumeAttachment>>, WarmPoolError> {
        retirement::homes(self, target, image).await
    }
    async fn retirement_drain_homes(
        &self,
        target: &PoolTarget,
        image: &str,
    ) -> Result<Option<Vec<layer3_adapter::HomeVolumeAttachment>>, WarmPoolError> {
        retirement::drain_homes(self, target, image).await
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
        match self.targets.insert_one(&target).await {
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

    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
    ) -> Result<PoolTarget, WarmPoolError> {
        let mut filter = self.scope();
        filter.extend(doc! {"_id":id,"revision":body.expected_revision as i64,"retiring":{"$ne":true},"retired":{"$ne":true},"lease_until":{"$lte":DateTime::now()}});
        let update = doc! {"$set":{"target_replicas":i64::from(body.target_replicas),
        "error":mongodb::bson::Bson::Null,"reconcile_after":DateTime::from_millis(0)},"$inc":{"revision":1_i64}};
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

    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.claim_matching(None).await
    }

    async fn claim_target(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.claim_matching(Some(id)).await
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
        let mut fields = doc! {"lease":mongodb::bson::Bson::Null,"lease_until":DateTime::from_millis(0),
        "reconcile_after":DateTime::from_millis(DateTime::now().timestamp_millis()+15_000),
        "error": if success { mongodb::bson::Bson::Null } else { "Native warm pool reconciliation is unavailable; the recorded target will retry.".into() }};
        if success {
            fields.insert("applied_revision", target.revision);
        }
        self.check_fenced_write(
            self.targets
                .update_one(fence(target), doc! {"$set":fields})
                .await,
        )
        .await
    }
}

impl MongoNativeWarmPools {
    async fn claim_matching(&self, id: Option<&str>) -> Result<Option<PoolTarget>, WarmPoolError> {
        let now = DateTime::now();
        let mut filter = self.scope();
        filter.extend(
            doc! {"retired":{"$ne":true},"lease_until":{"$lte":now},"reconcile_after":{"$lte":now}},
        );
        if let Some(id) = id {
            filter.insert("_id", id);
            filter.insert("retiring", doc! {"$ne":true});
        }
        self.targets.find_one_and_update(filter, doc! {"$set":{
            "lease":uuid::Uuid::new_v4().to_string(),"lease_until":DateTime::from_millis(now.timestamp_millis()+60_000)}})
            .sort(doc! {"reconcile_after":1,"_id":1}).return_document(ReturnDocument::After)
            .await.map_err(|_| WarmPoolError::Unavailable)?.map(validate).transpose()
    }
}
