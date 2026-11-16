//! Operator-only exact head retirement. No provider or delivery client is held.
mod apply;
mod inspect;
#[path = "unresolved_model.rs"]
mod model;
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
pub(super) fn execution_error(_: crate::turns::routed::ExecutionError) -> Error {
    Error::Corrupt
}
