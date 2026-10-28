//! Transport-verified changes to an already admitted message keep its consumers.
use super::{
    launch::{LaunchOutcome, LaunchRequest},
    routing_plan::{ConsumerLaunch, PlanError, RoutedPlan},
    routing_store::{AdmittedPlan, EventIdentity},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    ids::IdempotencyKey,
    seal::{event_ref::activity_digest, EventRef, MasterKeyRing},
};
use tokio::sync::OwnedSemaphorePermit;

/// Construct only after a native transport proves the exact original message,
/// source, channel and actor facts. Public normalized fields never grant this.
pub(crate) struct VerifiedMessageEvent {
    pub request: LaunchRequest,
    pub source_bytes: Vec<u8>,
    pub received_at: i64,
    pub original: EventIdentity,
    /// Set only after proving a durable confirmed outbound receipt for this
    /// exact consumer, provider, partition, conversation and native target.
    pub outbound_consumer: Option<cmaeg_core::ids::ConsumerId>,
}

pub(crate) fn shape(activity: &Activity) -> bool {
    let ActivityKind::Event { ce, .. } = &activity.kind else {
        return false;
    };
    matches!(
        ce.r#type.as_str(),
        "cma.message.updated.v1" | "cma.message.deleted.v1" | "cma.message.reaction.updated.v1"
    ) && ce.id == activity.event_id.as_str()
        && ce
            .subject
            .as_ref()
            .is_some_and(|subject| !subject.is_empty())
        && ce
            .data
            .as_ref()
            .and_then(|data| data["original_event_id"].as_str())
            .is_some_and(|id| !id.is_empty() && id != activity.event_id.as_str())
        && activity.routing.as_ref().is_some_and(|routing| {
            routing.source_id == activity.source.source_id()
                && routing.addressing_bytes == 0
                && routing.causation.is_none()
        })
}

pub(super) fn bind(
    request: LaunchRequest,
    original: &AdmittedPlan,
    keys: &MasterKeyRing,
    now: i64,
) -> Result<(LaunchRequest, RoutedPlan), PlanError> {
    bind_selected(request, original, keys, now, None)
}

/// Caller must first verify a durable confirmed-send receipt for this exact
/// consumer, original plan, provider, partition, conversation and native target.
/// This selector narrows retained authority; it never reruns current routing.
pub(super) fn bind_outbound(
    request: LaunchRequest,
    original: &AdmittedPlan,
    keys: &MasterKeyRing,
    now: i64,
    consumer: &cmaeg_core::ids::ConsumerId,
) -> Result<(LaunchRequest, RoutedPlan), PlanError> {
    bind_selected(request, original, keys, now, Some(consumer))
}

fn bind_selected(
    mut request: LaunchRequest,
    original: &AdmittedPlan,
    keys: &MasterKeyRing,
    now: i64,
    selected: Option<&cmaeg_core::ids::ConsumerId>,
) -> Result<(LaunchRequest, RoutedPlan), PlanError> {
    let ActivityKind::Event { ce, .. } = &request.activity.kind else {
        return Err(PlanError::NotOrdinary);
    };
    let definition = &request.channel.definition;
    let event = request
        .event_ref
        .open(keys, now)
        .map_err(|_| PlanError::InvalidReference)?
        .payload;
    if (selected.is_none() && !matches!(original.activity.kind, ActivityKind::Message { .. }))
        || original.identity.owner != definition.owner.subject
        || original.identity.channel_id != request.channel.id
        || original.identity.record_id != definition.record_id
        || original.activity.source != request.activity.source
        || original.activity.conversation != request.activity.conversation
        || original.retained_until <= now
        || definition.routing.is_none()
        || definition.source_of(&request.activity.source) != Some(&original.source)
        || ce
            .data
            .as_ref()
            .and_then(|data| data["original_event_id"].as_str())
            != Some(original.identity.event_id.as_str())
        || !event.verifies_activity(&request.activity)
        || event.channel_id != request.channel.id
        || event.record_id != definition.record_id
        || event.consumer.is_some()
        || event.lineage.is_some()
        || request.activity.routing.is_some()
        || request.consumer.is_some()
    {
        return Err(PlanError::ContextMismatch);
    }
    // The original selection remains the reason these exact consumers receive
    // the notification. Edited addressing never selects another consumer.
    let mut routing = original
        .activity
        .routing
        .clone()
        .ok_or(PlanError::ContextMismatch)?;
    routing.addressing_bytes = 0;
    if let Some(consumer) = selected {
        routing.targets.retain(|target| &target.id == consumer);
    }
    request.activity.routing = Some(routing);
    if !shape(&request.activity) || request.activity.validate().is_err() {
        return Err(PlanError::ContextMismatch);
    }
    let mut targets = Vec::new();
    for target in &original.targets {
        let retained = &target.launch;
        if selected.is_some_and(|consumer| &retained.consumer != consumer) {
            continue;
        }
        let consumer = request
            .channel
            .consumer(Some(&retained.consumer))
            .filter(|consumer| {
                !consumer.pending
                    && consumer.credential.is_some()
                    && consumer.provider == &retained.provider
            })
            .ok_or(PlanError::ConsumerUnavailable)?;
        let mut next = event.clone();
        next.activity_sha256 = Some(activity_digest(&request.activity));
        next.partition_key = retained.event.partition_key.clone();
        next.conversation_id = retained.event.conversation_id.clone();
        next.consumer = (!retained.consumer.is_default()).then(|| retained.consumer.clone());
        targets.push(ConsumerLaunch {
            history_custody: None,
            consumer: retained.consumer.clone(),
            provider: consumer.provider.clone(),
            idempotency_key: if retained.consumer.is_default() {
                IdempotencyKey::from_event(&request.activity.event_id)
            } else {
                IdempotencyKey::for_consumer(&request.activity.event_id, &retained.consumer)
            },
            event: next,
        });
    }
    let primary = targets.first().ok_or(PlanError::ContextMismatch)?;
    request.conversation_id = primary.event.conversation_id.clone();
    request.consumer = primary.event.consumer.clone();
    request.idempotency_key = primary.idempotency_key.clone();
    request.event_ref =
        EventRef::seal(&primary.event, keys).map_err(|_| PlanError::InvalidReference)?;
    let plan = RoutedPlan {
        activity: request.activity.clone(),
        source: original.source.clone(),
        targets,
        trace: Default::default(),
    };
    Ok((request, plan))
}

impl super::IngressState {
    pub(crate) async fn launch_message_event_verified(
        &self,
        verified: VerifiedMessageEvent,
        hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        self.launch_retained_message_event(verified, hold).await
    }
}
