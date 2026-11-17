//! Operator-only exact head retirement. No provider or delivery client is held.
mod apply;
mod failure;
pub(crate) use failure::Failure;
mod history;
mod inspect;
mod lanes;
#[path = "unresolved_model.rs"]
mod model;
mod scan;
use super::{
    partition::Partitions,
    partition_model::{Head, Item},
    Error,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, PlanStore},
    turns::routed::{operator::Evidence, Journal},
};
pub(crate) use model::*;
pub use model::{Binding, Request};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct Operator {
    pub partitions: Partitions,
    pub plans: PlanStore,
    pub journal: Journal,
    seen: Arc<Mutex<std::collections::BTreeSet<String>>>,
}
pub(super) struct Selected {
    head: Head,
    head_retention: Option<i64>,
    lane: Option<(Head, u64, i64)>,
    item: Item,
    item_version: u64,
    plan: AdmittedPlan,
    target_index: usize,
    evidence: Evidence,
    effect: Option<(serde_json::Value, u64)>,
    candidate: Candidate,
    queued_effect_ids: Vec<String>,
}
impl Operator {
    pub fn new(partitions: Partitions, plans: PlanStore, journal: Journal) -> Self {
        Self {
            partitions,
            plans,
            journal,
            seen: Default::default(),
        }
    }
}
pub(super) fn identifier(domain: &str, value: &str) -> String {
    crate::connections::store::index_id(domain, &[value])
}
pub(super) fn hash(value: &impl serde::Serialize) -> Result<String, Error> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| Error::Corrupt)?)
    ))
}
pub(super) fn execution_error(error: crate::turns::routed::ExecutionError) -> Error {
    use crate::turns::routed::ExecutionError as E;
    match error {
        E::Expired => Error::Expired,
        E::Busy => Error::Busy,
        E::LostLease => Error::Lost,
        E::Unavailable | E::Uncertain => Error::Unavailable,
        _ => Error::Corrupt,
    }
}
pub(super) fn admission_error(error: crate::ingress::routing_store::AdmissionError) -> Error {
    use crate::ingress::routing_store::AdmissionError as E;
    match error {
        E::Expired => Error::Expired,
        E::Unavailable | E::Uncertain => Error::Unavailable,
        _ => Error::Corrupt,
    }
}

#[cfg(test)]
mod lane_scan_tests;
