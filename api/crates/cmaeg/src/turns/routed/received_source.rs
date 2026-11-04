//! Original received bytes are available only through the exact durable target binding.
use super::{
    dispatcher_authority::validate_current, model::delivery_event, ExecutionError, Journal,
};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{AdmittedPlan, PlanStore},
    records::RecordClient,
};
use cmaeg_core::{
    activity::ActivityKind,
    seal::{EventRefPayload, MasterKeyRing},
};
use std::sync::Arc;

pub(crate) async fn retained_received_source(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    current: &ResolvedChannel,
    event: &EventRefPayload,
) -> Result<AdmittedPlan, ExecutionError> {
    if event.lineage.is_some() {
        return Err(ExecutionError::Invalid);
    }
    let journal = Journal::new(records.clone(), keys.clone());
    let binding = journal
        .tool_reply_binding(event)
        .await?
        .ok_or(ExecutionError::Invalid)?;
    let plan = PlanStore::new(records, keys)
        .load(&binding.identity)
        .await
        .map_err(super::dispatcher::admission_error)?
        .ok_or(ExecutionError::Invalid)?;
    if plan.admission_id != binding.root
        || plan.request_hash != binding.request_hash
        || !matches!(plan.activity.kind, ActivityKind::Message { .. })
        || !event.verifies_activity(&plan.activity)
    {
        return Err(ExecutionError::Corrupt);
    }
    let target = plan
        .targets
        .iter()
        .find(|t| t.id == binding.target)
        .ok_or(ExecutionError::Invalid)?;
    if delivery_event(&plan, target) != *event {
        return Err(ExecutionError::Invalid);
    }
    validate_current(current, &plan, target)?;
    if event.expires_at <= (journal.clock)() {
        return Err(ExecutionError::Expired);
    }
    Ok(plan)
}
