//! Exact-head operator disposition; no provider or delivery operation occurs here.
use super::{
    partition::Partitions,
    partition_model::{item_id, partition_id, Authority, Item},
    store::Write,
    Error,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    records::mongo::LeaseCondition,
    records::mongo::{ReplicaKind, ReplicaProjection},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRequest {
    pub partition_id: String,
    pub expected_head: String,
    pub expected_epoch: u64,
    pub expected_version: u64,
    pub operation_id: String,
    pub provider_terminal_confirmed: bool,
    pub suppress_delayed_replies: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseResult {
    pub operation_id: String,
    pub released_head: String,
    pub disposition: String,
    pub epoch: u64,
    pub released_at_ms: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Audit {
    kind: String,
    request: ReleaseRequest,
    owner: String,
    channel_id: String,
    actor_token_hash: String,
    admission_id: String,
    request_hash: String,
    actor_subject: String,
    original_phase: serde_json::Value,
    result: ReleaseResult,
}

/// Typed callers validate and freeze the original journal before this storage seam.
pub(crate) struct ReleaseRecord {
    pub id: String,
    pub version: u64,
    pub retained_until: i64,
    pub expires_at: i64,
    pub phase: &'static str,
    pub unresolved: bool,
    pub effect: bool,
    pub original_phase: serde_json::Value,
    pub abandoned: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
pub struct HeadPreview {
    pub partition_id: String,
    pub head: String,
    pub epoch: u64,
    pub version: u64,
    pub phase: String,
    pub event_expires_at: i64,
    pub releasable: bool,
}

pub(super) fn audit_id(partition: &str, target: &str) -> String {
    // The permanent tombstone is keyed by the original work, not just a caller's
    // retry key. Retrying admission cannot resurrect it after short-term TTLs.
    crate::connections::store::index_id("cmaeg-unresolved-release-v1", &[partition, target])
}

impl Partitions {
    pub(crate) async fn unresolved_item(&self, id: &str, owner: &str) -> Result<Item, Error> {
        let (head, _) = self.head(id, owner).await?;
        let front = head.queue.front().ok_or(Error::Lost)?;
        let (item, _) = self
            .store
            .read::<Item>(&item_id(front), true)
            .await?
            .ok_or(Error::Expired)?;
        if item.partition != id
            || item.work_id != *front
            || item.identity.owner != owner
            || item.done
        {
            return Err(Error::Corrupt);
        }
        Ok(item)
    }

    pub(crate) async fn release_receipt(
        &self,
        channel: &str,
        owner: &str,
        actor: &str,
        request: &ReleaseRequest,
    ) -> Result<Option<ReleaseResult>, Error> {
        let Some((audit, _)) = self
            .store
            .read::<Audit>(
                &audit_id(&request.partition_id, &request.expected_head),
                false,
            )
            .await?
        else {
            return Ok(None);
        };
        if audit.kind != "cmaeg_unknown_abandoned_v1"
            || audit.owner != owner
            || audit.channel_id != channel
            || audit.actor_token_hash != actor
            || audit.request != *request
        {
            return Err(Error::Lost);
        }
        Ok(Some(audit.result))
    }

    pub(crate) async fn preview_unknown(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        state: &ReleaseRecord,
    ) -> Result<HeadPreview, Error> {
        let id = partition_id(&self.membership.deployment, plan, target);
        let (head, version) = self.head(&id, &plan.identity.owner).await?;
        let now = self.store.clock().await?.millis;
        let front = head.queue.front().ok_or(Error::Lost)?;
        if *front != state.id {
            return Err(Error::Lost);
        }
        Ok(HeadPreview {
            partition_id: id,
            head: front.clone(),
            epoch: head.epoch,
            version,
            phase: state.phase.into(),
            event_expires_at: state.expires_at,
            releasable: state.unresolved
                && state.expires_at <= now.div_euclid(1000)
                && head.authority.as_ref().is_none_or(|a| a.until_ms <= now),
        })
    }

    pub(crate) async fn abandon_unknown(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        state: &ReleaseRecord,
        actor: &str,
        actor_subject: &str,
        request: &ReleaseRequest,
    ) -> Result<ReleaseResult, Error> {
        let id = partition_id(&self.membership.deployment, plan, target);
        if request.partition_id != id
            || request.expected_head != state.id
            || !request.provider_terminal_confirmed
            || !request.suppress_delayed_replies
            || !state.unresolved
            || state.version == 0
        {
            return Err(Error::Lost);
        }
        let (mut head, version) = self.head(&id, &plan.identity.owner).await?;
        if head.queue.front() != Some(&state.id)
            || head.epoch != request.expected_epoch
            || version != request.expected_version
        {
            return Err(Error::Lost);
        }
        let clock = self.store.clock().await?;
        if state.expires_at > clock.millis.div_euclid(1000)
            || state.retained_until <= clock.millis.div_euclid(1000)
        {
            return Err(Error::Expired);
        }
        if head
            .authority
            .as_ref()
            .is_some_and(|a| a.until_ms > clock.millis)
        {
            return Err(Error::Busy);
        }
        let predecessor = head
            .authority
            .as_ref()
            .map(|a| LeaseCondition::Expired(a.until_ms));
        head.epoch = head.epoch.checked_add(1).ok_or(Error::Corrupt)?;
        let token = Authority {
            instance: self.membership.instance.clone(),
            claim: uuid::Uuid::new_v4().to_string(),
            epoch: head.epoch,
            until_ms: clock.millis + super::config::LEASE.as_millis() as i64,
        };
        head.authority = Some(token.clone());
        // Exact precondition is checked once. Losing this CAS cannot be retried
        // against the next head, even if another caller completed a release.
        let claimed_version = self
            .store
            .write(
                &id,
                &head.owner,
                &head,
                Write {
                    expected: Some(version),
                    condition: predecessor,
                    ..Default::default()
                },
            )
            .await?;
        let lease = super::partition::PartitionLease {
            id: id.clone(),
            owner: head.owner.clone(),
            target: target.id.clone(),
            work_id: state.id.clone(),
            token,
            deadline: clock.started + super::config::LEASE,
        };
        let key = item_id(&state.id);
        let (mut item, item_version) = self
            .store
            .read::<Item>(&key, true)
            .await?
            .ok_or(Error::Expired)?;
        let mut expected_item = Item::new(id.clone(), plan, target);
        expected_item.work_id = state.id.clone();
        expected_item.effect = state.effect;
        if item != expected_item {
            return Err(Error::Corrupt);
        }
        item.done = true;
        head.queue.pop_front();
        head.authority = None;
        let result = ReleaseResult {
            operation_id: request.operation_id.clone(),
            released_head: state.id.clone(),
            disposition: "unknown_abandoned".into(),
            epoch: head.epoch,
            released_at_ms: clock.millis,
        };
        let audit = Audit {
            kind: "cmaeg_unknown_abandoned_v1".into(),
            request: request.clone(),
            owner: plan.identity.owner.clone(),
            actor_token_hash: actor.into(),
            channel_id: plan.identity.channel_id.to_string(),
            admission_id: plan.admission_id.clone(),
            request_hash: plan.request_hash.clone(),
            actor_subject: actor_subject.into(),
            original_phase: state.original_phase.clone(),
            result: result.clone(),
        };
        let guard = self.guard(&lease).await?;
        // One transaction binds the audit/tombstone, original uncertain state,
        // item deduplication marker and exactly one FIFO pop. A concurrent append
        // or release conflicts; a lost acknowledgement is read back by operation.
        let writes = [
            self.store.prepare(
                &audit_id(&id, &state.id),
                &head.owner,
                &audit,
                Write {
                    guard: Some(&guard),
                    ..Default::default()
                },
            )?,
            self.store.prepare(
                &state.id,
                &head.owner,
                &state.abandoned,
                Write {
                    expected: Some(state.version),
                    retained_until: Some(state.retained_until),
                    projection: state.effect.then(|| ReplicaProjection {
                        kind: ReplicaKind::Effect,
                        deployment: self.membership.deployment.clone(),
                        active: false,
                    }),
                    ..Default::default()
                },
            )?,
            self.store.prepare(
                &key,
                &head.owner,
                &item,
                Write {
                    expected: Some(item_version),
                    retained_until: Some(item.retained_until),
                    ..Default::default()
                },
            )?,
            self.store.prepare(
                &id,
                &head.owner,
                &head,
                Write {
                    expected: Some(claimed_version),
                    condition: Some(LeaseCondition::Live(lease.token.until_ms)),
                    projection: Some(self.projection(!head.queue.is_empty())),
                    ..Default::default()
                },
            )?,
        ];
        let committed = self.store.batch(&writes).await;
        match self
            .release_receipt(
                plan.identity.channel_id.as_str(),
                &head.owner,
                actor,
                request,
            )
            .await?
        {
            Some(receipt) => Ok(receipt),
            None => Err(committed.err().unwrap_or(Error::Unavailable)),
        }
    }

    pub(super) async fn was_abandoned(&self, partition: &str, target: &str) -> Result<bool, Error> {
        let Some((audit, _)) = self
            .store
            .read::<Audit>(&audit_id(partition, target), false)
            .await?
        else {
            return Ok(false);
        };
        if audit.kind != "cmaeg_unknown_abandoned_v1"
            || audit.request.partition_id != partition
            || audit.request.expected_head != target
            || audit.result.released_head != target
        {
            return Err(Error::Corrupt);
        }
        Ok(true)
    }
}
