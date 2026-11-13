//! Indexed active frontier discovery; retained receipt history is not the hot path.
use super::{
    partition::Partitions,
    partition_model::{item_id, Head, Item},
    Error,
};
use crate::{
    ingress::routing_store::{AdmissionError, PlanStore, RecoveryPage},
    records::mongo::ReplicaKind,
    turns::routed::scheduler::RecoverySource,
};
use futures_util::future::BoxFuture;
use std::{collections::BTreeMap, sync::Arc};
use tokio::{sync::Mutex, time::Instant};

pub(super) struct Snapshot {
    at: Instant,
    heads: BTreeMap<String, Head>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            at: Instant::now() - std::time::Duration::from_secs(1),
            heads: BTreeMap::new(),
        }
    }
}
pub(super) type Cache = Arc<Mutex<Snapshot>>;

impl Partitions {
    pub(super) async fn active_heads(&self) -> Result<BTreeMap<String, Head>, Error> {
        let mut snapshot = self.snapshot.lock().await;
        if snapshot.at.elapsed() < std::time::Duration::from_millis(100) {
            return Ok(snapshot.heads.clone());
        }
        let mut heads = BTreeMap::new();
        let mut cursor = None;
        loop {
            let rows = self
                .store
                .active::<Head>(
                    ReplicaKind::Partition,
                    &self.membership.deployment,
                    cursor.as_deref(),
                    128,
                )
                .await?;
            let count = rows.len();
            for (id, head, _) in rows {
                head.validate(&id, &self.membership.deployment, &head.owner)?;
                if head.queue.is_empty() {
                    return Err(Error::Corrupt);
                }
                cursor = Some(id.clone());
                heads.insert(id, head);
            }
            if heads.len() > 4096 {
                return Err(Error::Capacity);
            }
            if count < 128 {
                break;
            }
        }
        snapshot.at = Instant::now();
        snapshot.heads = heads.clone();
        Ok(heads)
    }

    pub(super) async fn assigned(&self, id: &str) -> Result<bool, Error> {
        let heads = self.active_heads().await?;
        let mut keys: Vec<_> = heads.into_keys().collect();
        if !keys.iter().any(|key| key == id) {
            keys.push(id.into());
        }
        Ok(self.membership.assignments(keys)?.get(id) == Some(&self.membership.instance))
    }
}

#[derive(Clone)]
pub(crate) struct Frontier {
    pub partitions: Partitions,
    pub plans: PlanStore,
}
impl RecoverySource for Frontier {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, AdmissionError>> {
        Box::pin(async move {
            let heads = self
                .partitions
                .active_heads()
                .await
                .map_err(admission_error)?;
            let assigned = self
                .partitions
                .membership
                .assignments(heads.keys().cloned())
                .map_err(admission_error)?;
            let selected: Vec<_> = heads
                .iter()
                .filter(|(id, _)| after.is_none_or(|after| id.as_str() > after))
                .filter(|(id, _)| assigned.get(*id) == Some(&self.partitions.membership.instance))
                .take(32)
                .collect();
            let next_cursor = (selected.len() == 32).then(|| selected.last().unwrap().0.clone());
            let mut plans = Vec::new();
            for (id, head) in selected {
                let target = head.queue.front().ok_or(AdmissionError::Corrupt)?;
                let (item, _) = self
                    .partitions
                    .store
                    .read::<Item>(&item_id(target), true)
                    .await
                    .map_err(admission_error)?
                    .ok_or(AdmissionError::Expired)?;
                if item.partition != *id || item.work_id != *target || item.done {
                    return Err(AdmissionError::Corrupt);
                }
                let mut plan = self
                    .plans
                    .load(&item.identity)
                    .await?
                    .ok_or(AdmissionError::Corrupt)?;
                if plan.request_hash != item.request_hash {
                    return Err(AdmissionError::Corrupt);
                }
                // Scheduling receives only the head target. The authoritative
                // immutable full plan is loaded again by the dispatcher.
                plan.targets.retain(|candidate| candidate.id == item.target);
                if plan.targets.len() != 1 {
                    return Err(AdmissionError::Corrupt);
                }
                plan.targets[0].id = item.work_id;
                plans.push(plan);
            }
            Ok(RecoveryPage {
                plans,
                issues: Vec::new(),
                next_cursor,
            })
        })
    }
}

pub(super) fn admission_error(error: Error) -> AdmissionError {
    match error {
        Error::Expired => AdmissionError::Expired,
        Error::Corrupt => AdmissionError::Corrupt,
        Error::Capacity => AdmissionError::TooLarge,
        Error::Busy | Error::Lost | Error::Unavailable => AdmissionError::Unavailable,
    }
}
