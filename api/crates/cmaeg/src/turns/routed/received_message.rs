//! Resolve received-message authority through the immutable original routed target.
use super::{
    dispatcher::{admission_error, current_target},
    model::delivery_event,
    Authority, ExecutionError, Journal,
};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, ingress::routing_store::PlanStore,
    records::RecordClient,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    ids::ConsumerId,
    seal::{EventRefPayload, MasterKeyRing},
};
use std::sync::Arc;

/// Prove the original received event and selected consumer before minting effects.
/// Callers may omit optional capability metadata on any failure; this performs
/// no provider or platform write and never falls back to unretained authority.
pub async fn current_received_message(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    authority: &dyn Authority,
    caller: AgentCaller,
    event: &EventRefPayload,
    activity: &Activity,
) -> Result<Arc<ResolvedChannel>, ExecutionError> {
    if event.lineage.is_some()
        || !matches!(activity.kind, ActivityKind::Message { .. })
        || !event.verifies_activity(activity)
    {
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
        .map_err(admission_error)?
        .ok_or(ExecutionError::Invalid)?;
    if plan.admission_id != binding.root
        || plan.request_hash != binding.request_hash
        || plan.activity != *activity
    {
        return Err(ExecutionError::Corrupt);
    }
    let consumer = event
        .consumer
        .clone()
        .unwrap_or_else(ConsumerId::default_consumer);
    let target = plan
        .targets
        .iter()
        .find(|target| target.id == binding.target && target.launch.consumer == consumer)
        .ok_or(ExecutionError::Invalid)?;
    if delivery_event(&plan, target) != *event {
        return Err(ExecutionError::Invalid);
    }
    // Reload after the durable reads; source admission and the original provider
    // must still match. Credentials are current, as in normal routed execution.
    let current = current_target(authority, &plan, target).await?;
    if caller.consumer(&current).ok().as_ref() != Some(&target.launch.consumer) {
        return Err(ExecutionError::AuthorityChanged);
    }
    if event.expires_at <= (journal.clock)() {
        return Err(ExecutionError::Expired);
    }
    Ok(current)
}

/// Persist real routing records for tests that previously only sealed an event.
/// Supply an ordinary pre-routing request; select the resulting consumer to use.
#[cfg(test)]
pub(crate) async fn bind_received_message_fixture(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    request: &crate::ingress::launch::LaunchRequest,
    consumer: Option<&ConsumerId>,
) -> (Activity, EventRefPayload) {
    bind_received_message_fixture_with_source(
        records,
        keys,
        request,
        consumer,
        &serde_json::to_vec(&request.activity).unwrap(),
    )
    .await
}

/// Preserve native verified bytes when exercising a tool that inspects the original wire message.
#[cfg(test)]
pub(crate) async fn bind_received_message_fixture_with_source(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    request: &crate::ingress::launch::LaunchRequest,
    consumer: Option<&ConsumerId>,
    bytes: &[u8],
) -> (Activity, EventRefPayload) {
    use crate::ingress::{
        routing_plan::{plan, OrdinaryPlan, RoutingOptions},
        routing_store::Candidate,
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let providers = request
        .channel
        .definition
        .consumers()
        .map(|consumer| consumer.provider.slug.to_string())
        .collect();
    let OrdinaryPlan::Routed(routed) = plan(
        request,
        &keys,
        now,
        RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &providers,
        },
    )
    .unwrap() else {
        panic!("received-message fixture needs durable routing")
    };
    let candidate = Candidate::prepare(request, &routed, bytes, &keys, now).unwrap();
    let stored = PlanStore::new(records.clone(), keys.clone())
        .admit(&candidate)
        .await
        .unwrap();
    let selected = consumer
        .cloned()
        .unwrap_or_else(ConsumerId::default_consumer);
    let target = stored
        .targets
        .iter()
        .find(|target| target.launch.consumer == selected)
        .expect("fixture consumer must belong to original routing selection");
    Journal::new(records, keys)
        .bind_tool_reply(&stored, target)
        .await
        .unwrap();
    let event = delivery_event(&stored, target);
    (stored.activity, event)
}

#[cfg(test)]
#[path = "received_message_tests.rs"]
mod tests;
