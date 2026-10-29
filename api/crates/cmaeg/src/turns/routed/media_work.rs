//! A bounded active-head index, created after immutable input freeze and before
//! its first provider submission. Recovery scans active media heads only.
use super::{
    media_schedule::MediaSchedule, storage, ExecutionError, FrozenTurn, Journal, Phase, TargetState,
};
use crate::ingress::routing_store::{
    AdmissionError, AdmittedPlan, EventIdentity, PlanStore, RecoveryIssue, RecoveryPage,
    StoredTarget,
};
use cmaeg_core::ids::RecordId;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
const KIND: &str = "routed_media_work_v1";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MediaWorkHead {
    kind: String,
    id: String,
    owner: String,
    pub identity: EventIdentity,
    pub root: String,
    pub target_id: String,
    pub request_hash: String,
    pub frozen_body_sha256: String,
    pub event_expires_at: i64,
    pub retained_until: i64,
    pub gateway_generation: Option<u64>,
    pub schedule: MediaSchedule,
}
#[derive(Clone)]
pub(crate) struct MediaWorkIndex {
    index: Journal,
    ordinary: Journal,
    plans: PlanStore,
}
impl MediaWorkIndex {
    #[cfg(test)]
    pub(super) fn fixture(
        journal: Journal,
        plans: PlanStore,
        records: crate::records::RecordClient,
    ) -> Self {
        Self {
            index: Journal {
                records: records.interactions(),
                ..journal.clone()
            },
            ordinary: journal,
            plans,
        }
    }
    pub fn new(journal: Journal, plans: PlanStore) -> Self {
        Self {
            index: Journal {
                records: journal.records.clone().media_continuations(),
                ..journal.clone()
            },
            ordinary: journal,
            plans,
        }
    }
    pub async fn prepare(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        body: &FrozenTurn,
    ) -> Result<(), ExecutionError> {
        let Some(schedule) = MediaSchedule::read(body, plan, target)? else {
            return Ok(());
        };
        body.validate(plan, target, &self.ordinary.keys, (self.ordinary.clock)())?;
        let head = MediaWorkHead {
            kind: KIND.into(),
            id: id(target),
            owner: plan.identity.owner.clone(),
            identity: plan.identity.clone(),
            root: plan.admission_id.clone(),
            target_id: target.id.clone(),
            request_hash: plan.request_hash.clone(),
            frozen_body_sha256: storage::hash(&storage::bytes(body)?),
            event_expires_at: target.launch.event.expires_at,
            retained_until: plan.retained_until,
            gateway_generation: body.binding_generation,
            schedule,
        };
        let (state, _) = self
            .ordinary
            .read::<TargetState>(&target.id)
            .await?
            .ok_or(ExecutionError::Invalid)?;
        validate_parent(&head, plan, target, &state)?;
        if !matches!(
            state.phase,
            Phase::Prepared | Phase::Starting | Phase::Unknown
        ) {
            return Err(ExecutionError::Invalid);
        }
        // Lost acknowledgements replay only the same immutable indexed head.
        self.index
            .write(&head.id, &head.owner, head.retained_until, &head, None)
            .await?;
        Ok(())
    }
    pub(super) async fn load(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<MediaWorkHead>, ExecutionError> {
        let Some((head, version)) = self.index.read::<MediaWorkHead>(&id(target)).await? else {
            return Ok(None);
        };
        if version != 1 {
            return Err(ExecutionError::Corrupt);
        }
        validate_head(&head, plan, target)?;
        Ok(Some(head))
    }
    /// Deleting an active head is permitted only after the caller has written
    /// durable terminal/refusal evidence to every indexed attachment ledger.
    pub(super) async fn finish(&self, head: &MediaWorkHead) -> Result<(), ExecutionError> {
        self.index
            .records
            .delete(
                &RecordId::new(&head.id).map_err(|_| ExecutionError::Corrupt)?,
                1,
            )
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        Ok(())
    }
    async fn page(&self, after: Option<&str>) -> Result<RecoveryPage, AdmissionError> {
        let heads = self
            .index
            .records
            .list(None, after, 32)
            .await
            .map_err(|_| AdmissionError::Unavailable)?;
        if heads.len() > 32 {
            return Err(AdmissionError::Corrupt);
        }
        let mut previous = after;
        for head in &heads {
            if previous.is_some_and(|value| head.record_id.as_str() <= value) {
                return Err(AdmissionError::Corrupt);
            }
            previous = Some(&head.record_id);
        }
        let next_cursor = (heads.len() == 32).then(|| heads.last().unwrap().record_id.clone());
        let mut page = RecoveryPage {
            plans: Vec::new(),
            issues: Vec::new(),
            next_cursor,
        };
        for entry in heads {
            let recovered = async {
                let Some((head, version)) = self
                    .index
                    .read::<MediaWorkHead>(&entry.record_id)
                    .await
                    .map_err(admission_error)?
                else {
                    return Ok(None);
                };
                if version != 1 || head.id != entry.record_id || head.owner != entry.owner {
                    return Err(AdmissionError::Corrupt);
                }
                let mut plan = self
                    .plans
                    .load(&head.identity)
                    .await?
                    .ok_or(AdmissionError::Corrupt)?;
                let target = plan
                    .targets
                    .iter()
                    .find(|target| target.id == head.target_id)
                    .ok_or(AdmissionError::Corrupt)?;
                validate_head(&head, &plan, target).map_err(admission_error)?;
                // Scheduling is only a wake-up. The executor reloads original
                // complete plan, source authorization and submitted checkpoint.
                plan.targets.retain(|target| target.id == head.target_id);
                Ok(Some(plan))
            }
            .await;
            match recovered {
                Ok(Some(plan)) => page.plans.push(plan),
                Ok(None) => {}
                Err(error) => page.issues.push(RecoveryIssue {
                    record_id: entry.record_id,
                    error,
                }),
            }
        }
        Ok(page)
    }
}
#[cfg(test)]
#[path = "media_work_tests.rs"]
mod tests;
impl super::scheduler::RecoverySource for MediaWorkIndex {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, AdmissionError>> {
        Box::pin(self.page(after))
    }
}
pub(super) fn validate_parent(
    head: &MediaWorkHead,
    plan: &AdmittedPlan,
    target: &StoredTarget,
    state: &TargetState,
) -> Result<(), ExecutionError> {
    validate_head(head, plan, target)?;
    super::journal::validate(state, plan, target)?;
    let generation = state
        .binding_preparation
        .as_ref()
        .map(|value| value.resolved_generation())
        .transpose()?
        .flatten();
    if generation != head.gateway_generation
        || state
            .body
            .as_ref()
            .is_none_or(|body| body.hash != head.frozen_body_sha256)
    {
        return Err(ExecutionError::Corrupt);
    }
    Ok(())
}
fn validate_head(
    head: &MediaWorkHead,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<(), ExecutionError> {
    if head.kind != KIND
        || head.id != id(target)
        || head.owner != plan.identity.owner
        || head.identity != plan.identity
        || head.root != plan.admission_id
        || head.target_id != target.id
        || head.request_hash != plan.request_hash
        || head.retained_until != plan.retained_until
        || head.event_expires_at != target.launch.event.expires_at
        || head.frozen_body_sha256.len() != 64
        || head.gateway_generation == Some(0)
    {
        return Err(ExecutionError::Corrupt);
    }
    head.schedule.validate(plan, target)
}
fn id(target: &StoredTarget) -> String {
    storage::identifier(&(KIND, &target.id))
}
fn admission_error(error: ExecutionError) -> AdmissionError {
    match error {
        ExecutionError::Unavailable | ExecutionError::Uncertain => AdmissionError::Unavailable,
        _ => AdmissionError::Corrupt,
    }
}
