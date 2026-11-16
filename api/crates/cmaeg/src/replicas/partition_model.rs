//! Sealed queue records retain admission identity and permanent epoch watermarks.
use super::Error;
use crate::{
    connections::store::index_id,
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

pub(super) const HEAD_KIND: &str = "cmaeg_partition_v1";
pub(super) const ITEM_KIND: &str = "cmaeg_partition_item_v1";
pub(super) const MAX_PENDING: usize = 512;

/// Deliberately excludes event, target, body, epoch and process identity. Routing
/// already canonicalizes conversation/partition; changing credentials must not
/// create a second serialization lane for the same authenticated conversation.
pub(crate) fn partition_id(deployment: &str, plan: &AdmittedPlan, target: &StoredTarget) -> String {
    index_id(
        "cmaeg-partition-v1",
        &[
            deployment,
            &plan.identity.owner,
            plan.identity.channel_id.as_str(),
            plan.identity.record_id.as_str(),
            target.launch.consumer.as_str(),
            target.launch.provider.slug.as_str(),
            target.launch.event.conversation_id.as_str(),
            &target.launch.event.partition_key,
        ],
    )
}

pub(super) fn item_id(target: &str) -> String {
    index_id("cmaeg-partition-item-v1", &[target])
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Authority {
    pub instance: String,
    pub claim: String,
    pub epoch: u64,
    pub until_ms: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Head {
    pub kind: String,
    pub id: String,
    pub deployment: String,
    pub owner: String,
    pub epoch: u64,
    pub authority: Option<Authority>,
    pub queue: VecDeque<String>,
}
impl Head {
    pub fn empty(id: String, deployment: String, owner: String) -> Self {
        Self {
            kind: HEAD_KIND.into(),
            id,
            deployment,
            owner,
            epoch: 0,
            authority: None,
            queue: VecDeque::new(),
        }
    }
    pub fn validate(&self, id: &str, deployment: &str, owner: &str) -> Result<(), Error> {
        if self.kind != HEAD_KIND
            || self.id != id
            || self.deployment != deployment
            || self.owner != owner
            || self.queue.len() > MAX_PENDING
            || self.queue.iter().collect::<BTreeSet<_>>().len() != self.queue.len()
            || self
                .queue
                .iter()
                .any(|target| uuid::Uuid::parse_str(target).is_err())
            || self.authority.as_ref().is_some_and(|authority| {
                self.queue.is_empty()
                    || authority.epoch != self.epoch
                    || authority.epoch == 0
                    || authority.instance.len() != 64
                    || uuid::Uuid::parse_str(&authority.claim).is_err()
            })
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
    pub fn matches(&self, target: &str, token: &Authority) -> bool {
        self.queue.front().is_some_and(|front| front == target)
            && self.authority.as_ref().is_some_and(|current| {
                current.instance == token.instance
                    && current.claim == token.claim
                    && current.epoch == token.epoch
                    && current.until_ms >= token.until_ms
            })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Item {
    pub kind: String,
    pub partition: String,
    pub identity: EventIdentity,
    pub target: String,
    pub work_id: String,
    pub effect: bool,
    pub request_hash: String,
    pub retained_until: i64,
    pub done: bool,
}
impl Item {
    pub fn new(partition: String, plan: &AdmittedPlan, target: &StoredTarget) -> Self {
        Self {
            kind: ITEM_KIND.into(),
            partition,
            identity: plan.identity.clone(),
            target: target.id.clone(),
            work_id: target.id.clone(),
            effect: false,
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
            done: false,
        }
    }
    pub fn same_admission(&self, expected: &Self) -> bool {
        let mut normalized = self.clone();
        normalized.done = false;
        normalized == *expected
    }
}
