use super::{model::*, storage};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    records::RecordClient,
    turns::execution_receipt::ExecutionVerifiers,
};
use cmaeg_core::seal::MasterKeyRing;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Journal {
    pub(super) start_measurement: Option<crate::telemetry::provider_start::Measurement>,
    pub(super) partitions: Option<crate::replicas::partition::Partitions>,
    pub(super) activity: Option<Arc<crate::activity::ActivityStore>>,
    pub(super) execution_verifiers: Arc<ExecutionVerifiers>,
    pub(super) records: RecordClient,
    pub(super) keys: Arc<MasterKeyRing>,
    pub(super) clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}
impl Journal {
    pub(super) fn receipts<'a>(
        &'a self,
        scope: &'a str,
        owner: &'a str,
        retained_until: i64,
    ) -> crate::delivery::retained_receipt::ReceiptStore<'a> {
        crate::delivery::retained_receipt::ReceiptStore {
            records: &self.records,
            keys: &self.keys,
            scope,
            owner,
            retained_until,
        }
    }

    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            partitions: None,
            start_measurement: None,
            activity: None,
            execution_verifiers: Arc::default(),
            records: records.interactions(),
            keys,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |value| value.as_secs().try_into().unwrap_or(i64::MAX))
            }),
        }
    }

    pub fn with_activity(mut self, activity: Arc<crate::activity::ActivityStore>) -> Self {
        self.activity = Some(activity);
        self
    }
    pub(super) async fn observe(
        &self,
        identity: &crate::ingress::routing_store::EventIdentity,
        change: crate::activity::Change,
    ) {
        if let Some(activity) = &self.activity {
            activity
                .observe(
                    identity.record_id.as_str(),
                    identity.event_id.as_str(),
                    &change,
                )
                .await;
        }
    }

    pub(super) async fn claim(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        worker: &str,
    ) -> Result<Lease, ExecutionError> {
        if worker.is_empty() || worker.len() > 128 {
            return Err(ExecutionError::Invalid);
        }
        let now = (self.clock)();
        if plan.retained_until <= now {
            return Err(ExecutionError::Expired);
        }
        let partition = match &self.partitions {
            Some(partitions) => Some(partitions.claim(plan, target).await?),
            None => None,
        };
        let existing = self.read::<TargetState>(&target.id).await?;
        let mut state = match existing {
            Some((mut state, version)) => {
                state.version = version;
                state
            }
            None => TargetState {
                kind: KIND.into(),
                id: target.id.clone(),
                root: plan.admission_id.clone(),
                owner: plan.identity.owner.clone(),
                request_hash: plan.request_hash.clone(),
                retained_until: plan.retained_until,
                event_expires_at: target.launch.event.expires_at,
                phase: Phase::Pending,
                binding_preparation: None,
                delivery: None,
                projection: None,
                body: None,
                ownership: Ownership {
                    worker: worker.into(),
                    claim: uuid::Uuid::new_v4().to_string(),
                    epoch: 0,
                    until: 0,
                },
                version: 0,
            },
        };
        validate(&state, plan, target)?;
        // Re-entry uses the opaque lease, not a worker name. Two concurrent
        // invocations from one process must never both own the same target.
        if partition.is_none() && state.version > 0 && state.ownership.until > now {
            return Err(ExecutionError::Busy);
        }
        state.ownership = Ownership {
            worker: worker.into(),
            claim: uuid::Uuid::new_v4().to_string(),
            epoch: state
                .ownership
                .epoch
                .checked_add(1)
                .ok_or(ExecutionError::Corrupt)?,
            until: partition
                .as_ref()
                .map_or(now + LEASE_SECONDS, |token| token.until_seconds()),
        };
        state.version = self
            .write_fenced(
                &partition,
                &state.id,
                &state.owner,
                state.retained_until,
                &state,
                (state.version > 0).then_some(state.version),
            )
            .await?;
        Ok(Lease { state, partition })
    }

    pub async fn guard(&self, lease: &Lease) -> Result<(), ExecutionError> {
        self.guard_partition(&lease.partition).await?;
        let now = (self.clock)();
        if lease.state.retained_until <= now {
            return Err(ExecutionError::Expired);
        }
        if lease.state.ownership.until <= now {
            return Err(ExecutionError::LostLease);
        }
        let Some((stored, version)) = self.read::<TargetState>(&lease.state.id).await? else {
            return Err(ExecutionError::LostLease);
        };
        if version != lease.state.version
            || storage::bytes(&stored)? != storage::bytes(&lease.state)?
        {
            return Err(ExecutionError::LostLease);
        }
        // Read latency also counts against ownership.
        if lease.state.ownership.until <= (self.clock)() {
            return Err(ExecutionError::LostLease);
        }
        Ok(())
    }

    pub async fn renew(&self, lease: &mut Lease) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        let mut next = lease.state.clone();
        if let (Some(partitions), Some(token)) = (&self.partitions, &mut lease.partition) {
            partitions.renew(token).await?;
            next.ownership.until = token.until_seconds();
        } else {
            next.ownership.until = (self.clock)() + LEASE_SECONDS;
        }
        self.commit(lease, next).await
    }

    pub async fn release(&self, lease: &mut Lease) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        let mut next = lease.state.clone();
        next.ownership.until = (self.clock)();
        self.commit(lease, next).await?;
        if let (Some(partitions), Some(token)) = (&self.partitions, &lease.partition) {
            if lease.state.phase.terminal() {
                partitions.complete(token).await?;
            } else {
                partitions.release(token).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn transition(
        &self,
        lease: &mut Lease,
        phase: Phase,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        let mut next = lease.state.clone();
        next.phase = phase;
        self.commit(lease, next).await
    }

    pub(super) async fn commit(
        &self,
        lease: &mut Lease,
        mut next: TargetState,
    ) -> Result<(), ExecutionError> {
        next.version = self
            .write_fenced(
                &lease.partition,
                &next.id,
                &next.owner,
                next.retained_until,
                &next,
                Some(lease.state.version),
            )
            .await?;
        lease.state = next;
        Ok(())
    }

    /// Requires the exact numeric event's durable downstream receipt. Merely
    /// observing SSE or enqueueing a send cannot satisfy this checkpoint.
    /// Unknown effects block progress until explicitly reconciled.
    pub async fn acknowledge_delivery(
        &self,
        lease: &mut Lease,
        response_id: &str,
        cursor: u64,
    ) -> Result<(), ExecutionError> {
        let Phase::Accepted {
            response_id: expected,
            durable_cursor,
        } = &lease.state.phase
        else {
            return Err(ExecutionError::Invalid);
        };
        if expected != response_id
            || durable_cursor.is_some_and(|previous| cursor < previous)
            || !lease.state.delivery.as_ref().is_some_and(|checkpoint| {
                checkpoint.response_id == response_id
                    && checkpoint.cursor == cursor
                    && checkpoint.status.settled()
            })
        {
            return Err(ExecutionError::Invalid);
        }
        self.transition(
            lease,
            Phase::Accepted {
                response_id: response_id.into(),
                durable_cursor: Some(cursor),
            },
        )
        .await
    }

    /// All provider events and platform effects must already be durably settled.
    pub async fn complete(
        &self,
        lease: &mut Lease,
        response_id: &str,
    ) -> Result<(), ExecutionError> {
        let Phase::Accepted {
            response_id: expected,
            durable_cursor,
        } = &lease.state.phase
        else {
            return Err(ExecutionError::Invalid);
        };
        if expected != response_id
            || !lease.state.delivery.as_ref().is_some_and(|checkpoint| {
                checkpoint.status.settled()
                    && Some(checkpoint.cursor) == *durable_cursor
                    && checkpoint.terminal
            })
        {
            return Err(ExecutionError::Invalid);
        }
        self.transition(
            lease,
            Phase::Completed {
                response_id: response_id.into(),
                durable_cursor: *durable_cursor,
            },
        )
        .await?;
        if let Some(measurement) = &self.start_measurement {
            measurement.completed(&lease.state.root);
        }
        Ok(())
    }
}

pub(super) fn validate(
    state: &TargetState,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<(), ExecutionError> {
    if state.kind != KIND
        || state.id != target.id
        || state.root != plan.admission_id
        || state.owner != plan.identity.owner
        || state.request_hash != plan.request_hash
        || state.retained_until != plan.retained_until
        || state.event_expires_at != target.launch.event.expires_at
        || state.ownership.worker.is_empty()
        || state.ownership.worker.len() > 128
        || uuid::Uuid::parse_str(&state.ownership.claim).is_err()
        || (!matches!(state.phase, Phase::Pending | Phase::Refused { .. }) && state.body.is_none())
        || (matches!(state.phase, Phase::Pending) && state.body.is_some())
        || (plan.prepares_binding(target)
            && !matches!(state.phase, Phase::Pending | Phase::Refused { .. })
            && state.binding_preparation.is_none())
    {
        return Err(ExecutionError::Corrupt);
    }
    if let Some(checkpoint) = &state.delivery {
        let response = match &state.phase {
            Phase::Accepted { response_id, .. } | Phase::Completed { response_id, .. } => {
                response_id
            }
            _ => return Err(ExecutionError::Corrupt),
        };
        if &checkpoint.response_id != response
            || checkpoint
                .execution_attribution
                .as_ref()
                .is_some_and(|attribution| {
                    !checkpoint.terminal
                        || !checkpoint.status.settled()
                        || !attribution.matches(target.launch.provider.slug.as_str(), response)
                })
            || [&checkpoint.frame_hash, &checkpoint.effect_hash]
                .iter()
                .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            || storage::bytes(checkpoint)?.len() > 20 * 1024
            || matches!(checkpoint.status, super::DeliveryStatus::Settled { .. })
                && !checkpoint.status.settled()
            || matches!(&checkpoint.status, super::DeliveryStatus::Partial { receipt }
                if !receipt.has_parts() || !matches!(receipt.delivery,
                    crate::delivery::intents::DeliveryOutcome::Unknown { .. }))
        {
            return Err(ExecutionError::Corrupt);
        }
    }
    if let Some(preparation) = &state.binding_preparation {
        preparation.validate(plan, target)?;
        if !matches!(state.phase, Phase::Pending | Phase::Refused { .. })
            && preparation.resolved_generation().is_err()
        {
            return Err(ExecutionError::Corrupt);
        }
    }
    match &state.phase {
        Phase::Accepted { response_id, .. } | Phase::Completed { response_id, .. }
            if !valid_response(response_id) =>
        {
            Err(ExecutionError::Corrupt)
        }
        Phase::Refused { code } if code.is_empty() || code.len() > 128 => {
            Err(ExecutionError::Corrupt)
        }
        _ => Ok(()),
    }
}
pub(super) fn valid_response(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "." | "..")
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}
