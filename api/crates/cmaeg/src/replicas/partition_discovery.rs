//! Indexed active frontier discovery; retained receipt history is not the hot path.
use super::{
    diagnostics::{Class, ScanError, Stage},
    partition::Partitions,
    partition_model::Head,
    Error,
};
use crate::{
    ingress::routing_store::{AdmissionError, PlanStore, RecoveryPage},
    records::mongo::ReplicaKind,
    turns::routed::scheduler::RecoverySource,
};
use futures_util::future::BoxFuture;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::{sync::Mutex, time::Instant};

pub(super) struct Snapshot {
    at: Instant,
    heads: BTreeMap<String, Head>,
    lanes: BTreeSet<String>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            at: Instant::now() - std::time::Duration::from_secs(1),
            heads: BTreeMap::new(),
            lanes: BTreeSet::new(),
        }
    }
}
pub(super) type Cache = Arc<Mutex<Snapshot>>;

impl Partitions {
    #[cfg(test)]
    pub(super) async fn active_heads(&self) -> Result<BTreeMap<String, Head>, Error> {
        Ok(self.active_snapshot().await?.0)
    }

    async fn active_snapshot(&self) -> Result<(BTreeMap<String, Head>, BTreeSet<String>), Error> {
        let mut snapshot = self.snapshot.lock().await;
        if snapshot.at.elapsed() < std::time::Duration::from_millis(100) {
            return Ok((snapshot.heads.clone(), snapshot.lanes.clone()));
        }
        let mut heads = BTreeMap::new();
        let mut lanes = BTreeSet::new();
        let mut active = BTreeSet::new();
        for ephemeral in [false, true] {
            let mut cursor = None;
            loop {
                let rows = self
                    .store
                    .active_entries_in::<Head>(
                        ReplicaKind::Partition,
                        &self.membership.deployment,
                        cursor.as_deref(),
                        128,
                        ephemeral,
                    )
                    .await
                    .inspect_err(|&error| {
                        self.isolation
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .report(&ScanError::new(error, Stage::HeadRead, None));
                    })?;
                let count = rows.len();
                for entry in rows {
                    let id = entry.id;
                    cursor = Some(id.clone());
                    let duplicate = !active.insert(id.clone());
                    let checked = entry
                        .value
                        .map_err(|error| ScanError::new(error, Stage::HeadDecode, Some(&id)))
                        .and_then(|head| {
                            head.validate(&id, &self.membership.deployment, &head.owner)
                                .map_err(|error| {
                                    ScanError::new(error, Stage::HeadValidate, Some(&id))
                                })?;
                            if head.queue.is_empty() {
                                return Err(ScanError::new(
                                    Class::Corrupt,
                                    Stage::Front,
                                    Some(&id),
                                ));
                            }
                            Ok(head)
                        });
                    let mut isolation = self.isolation.lock().unwrap_or_else(|e| e.into_inner());
                    if duplicate {
                        heads.remove(&id);
                        lanes.remove(&id);
                        isolation.failed(
                            &id,
                            &ScanError::new(Error::Corrupt, Stage::HeadValidate, Some(&id)),
                        );
                        continue;
                    }
                    match checked {
                        Ok(head) => {
                            heads.insert(id.clone(), head);
                            isolation.valid_head(&id);
                            if ephemeral {
                                lanes.insert(id.clone());
                            }
                        }
                        Err(error) => isolation.failed(&id, &error),
                    }
                }
                if heads.len() > 4096 {
                    self.isolation
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .report(&ScanError::new(Error::Capacity, Stage::HeadRead, None));
                    return Err(Error::Capacity);
                }
                if count < 128 {
                    break;
                }
            }
        }
        self.isolation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(&active);
        snapshot.at = Instant::now();
        snapshot.heads = heads.clone();
        snapshot.lanes = lanes.clone();
        Ok((heads, lanes))
    }

    pub(super) async fn assignments(
        &self,
        heads: &BTreeMap<String, Head>,
        lanes: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, String>, Error> {
        let mut assigned = self
            .membership
            .assignments(heads.keys().filter(|id| !lanes.contains(*id)).cloned())?;
        let lane_keys: Vec<_> = heads
            .keys()
            .filter(|id| lanes.contains(*id))
            .cloned()
            .collect();
        if !lane_keys.is_empty() {
            match self
                .membership
                .lane_assignments(&self.store, lane_keys)
                .await
            {
                Ok(lane_assigned) => assigned.extend(lane_assigned),
                Err(error) => {
                    self.isolation
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .report(&ScanError::new(error, Stage::Placement, None));
                }
            }
        }
        Ok(assigned)
    }

    pub(super) async fn assigned_custody(&self, id: &str, lane: bool) -> Result<bool, Error> {
        let (heads, lanes) = self.active_snapshot().await?;
        let mut keys: Vec<_> = heads
            .into_keys()
            .filter(|key| lanes.contains(key) == lane)
            .collect();
        if !keys.iter().any(|key| key == id) {
            keys.push(id.into());
        }
        let assigned = if lane {
            self.membership.lane_assignments(&self.store, keys).await?
        } else {
            self.membership.assignments(keys)?
        };
        Ok(assigned.get(id) == Some(&self.membership.instance))
    }
}

#[derive(Clone)]
pub(crate) struct Frontier {
    pub partitions: Partitions,
    pub plans: PlanStore,
}
impl RecoverySource for Frontier {
    fn start_measurement(&self) -> Option<crate::telemetry::provider_start::Measurement> {
        self.plans.start_measurement()
    }

    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, ScanError>> {
        Box::pin(async move {
            let (heads, lanes) = self
                .partitions
                .active_snapshot()
                .await
                .map_err(|e| ScanError::new(e, Stage::HeadRead, None))?;
            let assigned = self
                .partitions
                .assignments(&heads, &lanes)
                .await
                .map_err(|e| ScanError::new(e, Stage::Placement, None))?;
            let selected: Vec<_> = heads
                .iter()
                .filter(|(id, _)| after.is_none_or(|after| id.as_str() > after))
                .filter(|(id, _)| assigned.get(*id) == Some(&self.partitions.membership.instance))
                .take(32)
                .collect();
            let next_cursor = (selected.len() == 32).then(|| selected.last().unwrap().0.clone());
            let mut plans = Vec::new();
            let mut issues = Vec::new();
            for (id, head) in selected {
                if !self
                    .partitions
                    .isolation
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .ready(id)
                {
                    continue;
                }
                let result =
                    super::head_evidence::load(&self.partitions, &self.plans, id, head, true).await;
                let mut isolation = self
                    .partitions
                    .isolation
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(loaded) => {
                        isolation.healthy(id);
                        if let Some(mut loaded) = loaded {
                            // The dispatcher reloads the immutable complete plan.
                            let mut target = loaded.plan.targets.remove(loaded.target_index);
                            target.id = loaded.item.work_id;
                            loaded.plan.targets = vec![target];
                            plans.push(loaded.plan);
                        }
                    }
                    Err(error) => {
                        isolation.failed(id, &error);
                        issues.push(crate::ingress::routing_store::RecoveryIssue {
                            record_id: id.clone(),
                            error: error.admission(),
                            diagnostic: Some(error),
                        });
                    }
                }
            }
            Ok(RecoveryPage {
                plans,
                issues,
                next_cursor,
            })
        })
    }
}

#[cfg(test)]
#[path = "partition_discovery_tests.rs"]
mod tests;

pub(super) fn admission_error(error: Error) -> AdmissionError {
    match error {
        Error::Expired => AdmissionError::Expired,
        Error::Corrupt => AdmissionError::Corrupt,
        Error::Capacity => AdmissionError::TooLarge,
        Error::Busy | Error::Lost | Error::Unavailable => AdmissionError::Unavailable,
    }
}
