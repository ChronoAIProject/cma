//! Bounded target-specific custody using the existing expiring interaction store.
use super::{
    partition::Partitions,
    partition_model::{item_id, partition_id, Head, Item},
    store::Write,
    Error,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    records::mongo::{ReplicaKind, ReplicaProjection},
};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionTime {
    pub kind: String,
    pub work: String,
    pub target: String,
    pub admitted_at: i64,
}
pub(super) fn time_id(work: &str) -> String {
    crate::connections::store::index_id("delayed-lane-admission-v1", &[work])
}

pub(crate) fn id(deployment: &str, plan: &AdmittedPlan, target: &StoredTarget) -> String {
    crate::connections::store::index_id(
        "cmaeg-delayed-lane-v1",
        &[
            deployment,
            &plan.identity.owner,
            &plan.admission_id,
            &target.id,
        ],
    )
}
pub(crate) fn retention(plan: &AdmittedPlan, target: &StoredTarget) -> i64 {
    target
        .launch
        .event
        .expires_at
        .saturating_add(cmaeg_core::seal::REPLY_TARGET_TTL)
        .saturating_add(super::config::LEASE.as_secs() as i64 + 86400)
        .min(plan.retained_until)
}
impl Partitions {
    /// Bound a renewable peer's wait across worker retries. Lane admission
    /// already records this clock; a legacy FIFO first records its Pending wait.
    pub(crate) async fn delayed_pending_expired(
        &self,
        lease: &super::partition::PartitionLease,
        retained_until: i64,
    ) -> Result<bool, Error> {
        let key = time_id(&lease.work_id);
        let now = self.store.clock().await?.millis / 1000;
        if let Some((time, _)) = self.store.read::<AdmissionTime>(&key, true).await? {
            if time.kind != "delayed_lane_admission_v1"
                || time.work != lease.work_id
                || time.target != lease.target
                || time.admitted_at <= 0
                || time.admitted_at > now
            {
                return Err(Error::Corrupt);
            }
            return Ok(now - time.admitted_at >= 30);
        }
        if lease.is_lane() {
            return Err(Error::Corrupt);
        }
        let guard = self.guard(lease).await?;
        self.store
            .write(
                &key,
                &lease.owner,
                &AdmissionTime {
                    kind: "delayed_lane_admission_v1".into(),
                    work: lease.work_id.clone(),
                    target: lease.target.clone(),
                    admitted_at: now,
                },
                Write {
                    retained_until: Some(retained_until),
                    guard: Some(&guard),
                    ..Default::default()
                },
            )
            .await?;
        Ok(false)
    }

    pub(crate) async fn admit_delayed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work: &str,
        root: &impl Serialize,
    ) -> Result<(), Error> {
        if !self
            .membership
            .capabilities(&self.store)
            .await
            .is_ok_and(|members| members.iter().all(|m| m.delayed_lane))
        {
            self.write_effect_root(work, &plan.identity.owner, plan.retained_until, root)
                .await?;
            return Ok(());
        }
        let lane = id(&self.membership.deployment, plan, target);
        let until = retention(plan, target);
        for _ in 0..8 {
            let now = self.store.clock().await?.millis / 1000;
            if now >= until {
                return Err(Error::Expired);
            }
            let (mut head, version) = self
                .store
                .read::<Head>(&lane, true)
                .await?
                .map(|(head, version)| (head, Some(version)))
                .unwrap_or((
                    Head::empty(
                        lane.clone(),
                        self.membership.deployment.clone(),
                        plan.identity.owner.clone(),
                    ),
                    None,
                ));
            head.validate(&lane, &self.membership.deployment, &plan.identity.owner)?;
            if head.queue.len() >= super::partition_model::MAX_PENDING {
                return Err(Error::Capacity);
            }
            if head.queue.contains(&work.to_owned()) {
                return Err(Error::Corrupt);
            }
            head.queue.push_back(work.into());
            let mut item = Item::new(lane.clone(), plan, target);
            item.effect = true;
            item.work_id = work.into();
            item.retained_until = until;
            let admission_time = AdmissionTime {
                kind: "delayed_lane_admission_v1".into(),
                work: work.into(),
                target: target.id.clone(),
                admitted_at: now,
            };
            let writes = [
                self.store.prepare(
                    &time_id(work),
                    &plan.identity.owner,
                    &admission_time,
                    Write {
                        retained_until: Some(until),
                        ..Default::default()
                    },
                )?,
                self.store.prepare(
                    work,
                    &plan.identity.owner,
                    root,
                    Write {
                        retained_until: Some(plan.retained_until),
                        projection: Some(ReplicaProjection {
                            kind: ReplicaKind::Effect,
                            deployment: self.membership.deployment.clone(),
                            active: false,
                        }),
                        ..Default::default()
                    },
                )?,
                self.store.prepare(
                    &item_id(work),
                    &plan.identity.owner,
                    &item,
                    Write {
                        retained_until: Some(until),
                        ..Default::default()
                    },
                )?,
                self.store.prepare(
                    &lane,
                    &plan.identity.owner,
                    &head,
                    Write {
                        retained_until: Some(until),
                        expected: version,
                        projection: Some(self.projection(true)),
                        ..Default::default()
                    },
                )?,
            ];
            match self.store.batch(&writes).await {
                Ok(_) => return Ok(()),
                Err(Error::Busy) => {
                    if self
                        .store
                        .read::<serde_json::Value>(work, true)
                        .await?
                        .is_some()
                    {
                        return Err(Error::Busy);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::Busy)
    }
    /// Existing items pin custody even when the live cohort has changed. A root
    /// without an item predates atomic lane admission and remains in the FIFO.
    pub(crate) async fn ensure_delayed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work: &str,
    ) -> Result<(), Error> {
        if let Some((item, _)) = self.store.read::<Item>(&item_id(work), true).await? {
            self.validate_effect_item(plan, target, work, &item)?;
            return Ok(());
        }
        if self.store.clock().await?.millis / 1000 >= retention(plan, target) {
            return Err(Error::Expired);
        }
        // A lane publishes its item and timestamp atomically. Losing the item
        // cannot turn that retained occurrence into a legacy FIFO admission.
        if self
            .store
            .read::<AdmissionTime>(&time_id(work), true)
            .await?
            .is_some()
        {
            return Err(Error::Lost);
        }
        self.enqueue_effect(plan, target, work).await
    }
    pub(super) fn validate_effect_item(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work: &str,
        item: &Item,
    ) -> Result<Option<i64>, Error> {
        let original = partition_id(&self.membership.deployment, plan, target);
        let lane = id(&self.membership.deployment, plan, target);
        let until = if item.partition == original {
            None
        } else if item.partition == lane {
            Some(retention(plan, target))
        } else {
            return Err(Error::Corrupt);
        };
        let mut expected = Item::new(item.partition.clone(), plan, target);
        expected.work_id = work.into();
        expected.effect = true;
        expected.retained_until = until.unwrap_or(plan.retained_until);
        if !item.same_admission(&expected) {
            return Err(Error::Corrupt);
        }
        Ok(until)
    }
}
