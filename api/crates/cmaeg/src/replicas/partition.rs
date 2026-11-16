//! Exact captured partition authority. Placement never grants permission to write.
use super::{
    config::LEASE,
    membership::Membership,
    partition_model::{partition_id, Authority, Head},
    store::{Store, Write},
    Error,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    records::mongo::{LeaseCondition, ReplicaGuard, ReplicaKind, ReplicaProjection},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::time::Instant;

#[derive(Clone)]
pub(crate) struct Partitions {
    pub(super) store: Store,
    pub(super) membership: Membership,
    // Local captures are only a routing aid. Every use authenticates the durable
    // record and compares the exact claim, epoch, front target and deadline.
    pub(super) local: Arc<RwLock<HashMap<String, PartitionLease>>>,
    pub(super) snapshot: super::partition_discovery::Cache,
}

/// Intentionally opaque; Clone retains the deadline captured before an await.
/// Neither a worker name nor a later owner lookup can manufacture this receipt.
#[derive(Clone)]
pub struct PartitionLease {
    pub(super) id: String,
    pub(super) owner: String,
    pub(super) target: String,
    pub(super) work_id: String,
    pub(super) token: Authority,
    pub(super) deadline: Instant,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionFence {
    pub instance: String,
    pub epoch: u64,
    pub claim: String,
    pub until_ms: i64,
}
#[derive(Clone)]
pub struct PartitionOwner {
    pub origin: String,
    pub fence: PartitionFence,
}

impl PartitionLease {
    /// Snapshot only: this value cannot acquire or extend partition authority.
    pub fn fence(&self) -> PartitionFence {
        PartitionFence {
            instance: self.token.instance.clone(),
            epoch: self.token.epoch,
            claim: self.token.claim.clone(),
            until_ms: self.token.until_ms,
        }
    }

    pub fn matches(&self, expected: &PartitionFence) -> bool {
        self.live()
            && self.token.instance == expected.instance
            && self.token.epoch == expected.epoch
            && self.token.claim == expected.claim
            && self.token.until_ms >= expected.until_ms
    }

    pub(crate) fn until_seconds(&self) -> i64 {
        self.token.until_ms.div_euclid(1000)
    }
    pub(crate) fn live(&self) -> bool {
        Instant::now() < self.deadline
    }
}

impl Partitions {
    pub(crate) fn membership(&self) -> Membership {
        self.membership.clone()
    }

    pub(crate) fn clock(&self) -> Arc<dyn Fn() -> i64 + Send + Sync> {
        let membership = self.membership.clone();
        Arc::new(move || membership.now_seconds())
    }

    pub fn new(store: Store, membership: Membership) -> Self {
        Self {
            store,
            membership,
            local: Arc::new(RwLock::new(HashMap::new())),
            snapshot: Default::default(),
        }
    }

    pub(super) fn projection(&self, active: bool) -> ReplicaProjection {
        ReplicaProjection {
            kind: ReplicaKind::Partition,
            deployment: self.membership.deployment.clone(),
            active,
        }
    }

    pub(super) async fn head(&self, id: &str, owner: &str) -> Result<(Head, u64), Error> {
        let (head, version) = self
            .store
            .read::<Head>(id, false)
            .await?
            .ok_or(Error::Lost)?;
        head.validate(id, &self.membership.deployment, owner)?;
        Ok((head, version))
    }

    pub async fn claim(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<PartitionLease, Error> {
        self.claim_work(plan, target, &target.id).await
    }

    pub(super) async fn claim_work(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work_id: &str,
    ) -> Result<PartitionLease, Error> {
        let id = partition_id(&self.membership.deployment, plan, target);
        if !self.assigned(&id).await? {
            return Err(Error::Busy);
        }
        let (mut head, version) = self.head(&id, &plan.identity.owner).await?;
        if self.next_work(&head).await? != work_id {
            return Err(Error::Busy);
        }
        let clock = self.store.clock().await?;
        if plan.retained_until <= clock.millis.div_euclid(1000) {
            return Err(Error::Expired);
        }
        if head
            .authority
            .as_ref()
            .is_some_and(|a| a.until_ms > clock.millis)
        {
            return Err(Error::Busy);
        }
        let condition = head
            .authority
            .as_ref()
            .map(|a| LeaseCondition::Expired(a.until_ms));
        // A continuation takes its own claim after the previous authority has
        // ended. Moving just this child to the front retains the unresolved
        // parent and every unrelated successor in their existing order.
        if head.queue.front().is_some_and(|front| front != work_id) {
            let index = head
                .queue
                .iter()
                .position(|id| id == work_id)
                .ok_or(Error::Corrupt)?;
            let continuation = head.queue.remove(index).ok_or(Error::Corrupt)?;
            head.queue.push_front(continuation);
        }
        head.epoch = head.epoch.checked_add(1).ok_or(Error::Corrupt)?;
        let token = Authority {
            instance: self.membership.instance.clone(),
            claim: uuid::Uuid::new_v4().to_string(),
            epoch: head.epoch,
            until_ms: clock.millis + LEASE.as_millis() as i64,
        };
        head.authority = Some(token.clone());
        self.store
            .write(
                &id,
                &head.owner,
                &head,
                Write {
                    expected: Some(version),
                    condition,
                    projection: Some(self.projection(true)),
                    ..Default::default()
                },
            )
            .await?;
        let lease = PartitionLease {
            id,
            owner: head.owner,
            target: target.id.clone(),
            work_id: work_id.into(),
            token,
            deadline: clock.started + LEASE,
        };
        if !lease.live() {
            return Err(Error::Lost);
        }
        self.local
            .write()
            .map_err(|_| Error::Lost)?
            .insert(lease.id.clone(), lease.clone());
        Ok(lease)
    }

    pub async fn owner(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<PartitionOwner, Error> {
        let id = partition_id(&self.membership.deployment, plan, target);
        let (head, _) = self.head(&id, &plan.identity.owner).await?;
        if head.queue.front() != Some(&target.id) {
            return Err(Error::Busy);
        }
        let token = head.authority.ok_or(Error::Busy)?;
        if token.until_ms <= self.store.clock().await?.millis {
            return Err(Error::Lost);
        }
        let origin = self.membership.origin(&token.instance)?;
        Ok(PartitionOwner {
            origin,
            fence: PartitionFence {
                instance: token.instance,
                epoch: token.epoch,
                claim: token.claim,
                until_ms: token.until_ms,
            },
        })
    }

    pub async fn capture(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<PartitionLease, Error> {
        let id = partition_id(&self.membership.deployment, plan, target);
        let lease = self
            .local
            .read()
            .map_err(|_| Error::Lost)?
            .get(&id)
            .cloned()
            .ok_or(Error::Busy)?;
        if lease.target != target.id
            || lease.work_id != target.id
            || lease.owner != plan.identity.owner
        {
            return Err(Error::Busy);
        }
        self.guard(&lease).await?;
        Ok(lease)
    }

    /// The returned physical record version may change with queue insertions;
    /// the captured authority must still match byte-for-byte. No token adoption.
    pub async fn guard(&self, lease: &PartitionLease) -> Result<ReplicaGuard, Error> {
        if !lease.live() || lease.token.instance != self.membership.instance {
            return Err(Error::Lost);
        }
        let (head, version) = self.head(&lease.id, &lease.owner).await?;
        if !head.matches(&lease.work_id, &lease.token) || !lease.live() {
            return Err(Error::Lost);
        }
        Ok(ReplicaGuard {
            id: lease.id.clone(),
            owner: lease.owner.clone(),
            version,
            until_ms: lease.token.until_ms,
        })
    }

    pub async fn renew(&self, lease: &mut PartitionLease) -> Result<(), Error> {
        if !lease.live() {
            return Err(Error::Lost);
        }
        let (mut head, version) = self.head(&lease.id, &lease.owner).await?;
        if !head.matches(&lease.work_id, &lease.token) {
            return Err(Error::Lost);
        }
        let clock = self.store.clock().await?;
        if !lease.live() {
            return Err(Error::Lost);
        }
        let mut token = lease.token.clone();
        token.until_ms = clock.millis + LEASE.as_millis() as i64;
        head.authority = Some(token.clone());
        self.store
            .write(
                &lease.id,
                &lease.owner,
                &head,
                Write {
                    expected: Some(version),
                    condition: Some(LeaseCondition::Live(lease.token.until_ms)),
                    ..Default::default()
                },
            )
            .await?;
        lease.token = token;
        lease.deadline = clock.started + LEASE;
        if !lease.live() {
            return Err(Error::Lost);
        }
        self.local
            .write()
            .map_err(|_| Error::Lost)?
            .insert(lease.id.clone(), lease.clone());
        Ok(())
    }

    pub async fn release(&self, lease: &PartitionLease) -> Result<(), Error> {
        let (mut head, version) = self.head(&lease.id, &lease.owner).await?;
        if !head.matches(&lease.work_id, &lease.token) || !lease.live() {
            return Err(Error::Lost);
        }
        head.authority = None;
        self.store
            .write(
                &lease.id,
                &lease.owner,
                &head,
                Write {
                    expected: Some(version),
                    condition: Some(LeaseCondition::Live(lease.token.until_ms)),
                    ..Default::default()
                },
            )
            .await?;
        self.forget(lease)?;
        Ok(())
    }

    pub(super) fn forget(&self, lease: &PartitionLease) -> Result<(), Error> {
        let mut local = self.local.write().map_err(|_| Error::Lost)?;
        if local
            .get(&lease.id)
            .is_some_and(|current| current.token == lease.token)
        {
            local.remove(&lease.id);
        }
        Ok(())
    }
}
