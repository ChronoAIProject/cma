//! FIFO insertion linearizes before successful ingress acknowledgement.
use super::{
    partition::{PartitionLease, Partitions},
    partition_model::{item_id, partition_id, Head, Item, MAX_PENDING},
    store::Write,
    Error,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    records::mongo::LeaseCondition,
};

impl Partitions {
    pub async fn enqueue(&self, plan: &AdmittedPlan) -> Result<(), Error> {
        for target in &plan.targets {
            self.enqueue_target(plan, target).await?;
        }
        Ok(())
    }

    pub async fn enqueue_effect(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work_id: &str,
    ) -> Result<(), Error> {
        if work_id == target.id || self.store.records.record_url(work_id).is_err() {
            return Err(Error::Corrupt);
        }
        let mut item = Item::new(
            partition_id(&self.membership.deployment, plan, target),
            plan,
            target,
        );
        item.work_id = work_id.into();
        item.effect = true;
        self.enqueue_item(plan, item).await
    }

    async fn enqueue_target(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), Error> {
        self.enqueue_item(
            plan,
            Item::new(
                partition_id(&self.membership.deployment, plan, target),
                plan,
                target,
            ),
        )
        .await
    }

    async fn enqueue_item(&self, plan: &AdmittedPlan, expected: Item) -> Result<(), Error> {
        let id = expected.partition.clone();
        let item_key = item_id(&expected.work_id);
        for _ in 0..8 {
            let clock = self.store.clock().await?;
            if plan.retained_until <= clock.millis.div_euclid(1000) {
                return Err(Error::Expired);
            }
            if let Some((item, _)) = self.store.read::<Item>(&item_key, true).await? {
                return if item.same_admission(&expected) {
                    Ok(())
                } else {
                    Err(Error::Corrupt)
                };
            }
            let (mut head, version) = match self.store.read::<Head>(&id, false).await? {
                Some((head, version)) => (head, Some(version)),
                None => (
                    Head::empty(
                        id.clone(),
                        self.membership.deployment.clone(),
                        plan.identity.owner.clone(),
                    ),
                    None,
                ),
            };
            head.validate(&id, &self.membership.deployment, &plan.identity.owner)?;
            if head.queue.contains(&expected.work_id) {
                // These majority reads are not one snapshot. A competing
                // insertion may commit between the item miss and head read.
                // Require its exact retained item before acknowledging custody.
                let item = self.store.read::<Item>(&item_key, true).await?;
                let outcome = match item {
                    Some((item, _)) if item.same_admission(&expected) => "verified",
                    Some(_) => "mismatch",
                    None => "missing",
                };
                metrics::counter!("cmaeg_replica_admission_rereads_total", "outcome" => outcome)
                    .increment(1);
                return if outcome == "verified" {
                    Ok(())
                } else {
                    Err(Error::Corrupt)
                };
            }
            if head.queue.len() >= MAX_PENDING {
                return Err(Error::Capacity);
            }
            head.queue.push_back(expected.work_id.clone());
            let writes = [
                self.store.prepare(
                    &item_key,
                    &plan.identity.owner,
                    &expected,
                    Write {
                        retained_until: Some(plan.retained_until),
                        ..Default::default()
                    },
                )?,
                self.store.prepare(
                    &id,
                    &head.owner,
                    &head,
                    Write {
                        expected: version,
                        projection: Some(self.projection(true)),
                        ..Default::default()
                    },
                )?,
            ];
            match self.store.batch(&writes).await {
                Ok(_) => return Ok(()),
                Err(Error::Busy) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Busy)
    }

    /// Only a caller that proved terminal settlement may advance the frontier.
    /// The completion marker and head pop are one transaction, including the
    /// exact predecessor authority/expiry. Unknown and Partial never call this.
    pub async fn complete(&self, lease: &PartitionLease) -> Result<(), Error> {
        for _ in 0..8 {
            if !lease.live() {
                return Err(Error::Lost);
            }
            let (mut head, version) = self.head(&lease.id, &lease.owner).await?;
            if !head.matches(&lease.work_id, &lease.token) {
                return Err(Error::Lost);
            }
            let key = item_id(&lease.work_id);
            let (mut item, item_version) = self
                .store
                .read::<Item>(&key, true)
                .await?
                .ok_or(Error::Expired)?;
            if item.partition != lease.id
                || item.target != lease.target
                || item.work_id != lease.work_id
                || item.done
            {
                return Err(Error::Corrupt);
            }
            item.done = true;
            head.queue.pop_front();
            head.authority = None;
            let writes = [
                self.store.prepare(
                    &key,
                    &lease.owner,
                    &item,
                    Write {
                        expected: Some(item_version),
                        retained_until: Some(item.retained_until),
                        ..Default::default()
                    },
                )?,
                self.store.prepare(
                    &lease.id,
                    &lease.owner,
                    &head,
                    Write {
                        expected: Some(version),
                        condition: Some(LeaseCondition::Live(lease.token.until_ms)),
                        projection: Some(self.projection(!head.queue.is_empty())),
                        ..Default::default()
                    },
                )?,
            ];
            match self.store.batch(&writes).await {
                Ok(_) => {
                    self.forget(lease)?;
                    return Ok(());
                }
                Err(Error::Busy) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Busy)
    }
}

impl crate::ingress::routing_store::AdmissionQueue for Partitions {
    fn index_deployment(&self) -> Option<&str> {
        Some(&self.membership.deployment)
    }
    fn enqueue<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<(), crate::ingress::routing_store::AdmissionError>,
    > {
        Box::pin(async move {
            Partitions::enqueue(self, plan)
                .await
                .map_err(super::partition_discovery::admission_error)
        })
    }
}
