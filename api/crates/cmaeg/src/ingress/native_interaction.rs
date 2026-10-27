//! Verified native ingress alone may admit a card's immutable single consumer.
use super::{
    launch::LaunchRequest,
    routing_plan::{ConsumerLaunch, PlanError, RoutedPlan},
};
use cmaeg_core::{
    activity::{ActivityKind, RouteResolution, SourceKind},
    seal::MasterKeyRing,
};

pub(crate) fn shape(activity: &cmaeg_core::activity::Activity) -> bool {
    matches!(activity.kind, ActivityKind::Interaction(_))
        && matches!(
            activity.source,
            SourceKind::Discord { .. } | SourceKind::Whatsapp { .. } | SourceKind::NativeLark(_)
        )
        && activity.routing.as_ref().is_some_and(|routing| {
            routing.resolution == RouteResolution::Interaction
                && routing.targets.len() == 1
                && routing.source_id == activity.source.source_id()
                && routing.addressing_bytes == 0
                && routing.rule_id.is_none()
                && routing.causation.is_none()
        })
}
pub(crate) fn plan(
    request: &LaunchRequest,
    keys: &MasterKeyRing,
    now: i64,
) -> Result<RoutedPlan, PlanError> {
    if !shape(&request.activity) {
        return Err(PlanError::NotOrdinary);
    }
    let event = request
        .event_ref
        .open(keys, now)
        .map_err(|_| PlanError::InvalidReference)?
        .payload;
    let target = &request.activity.routing.as_ref().unwrap().targets[0].id;
    let consumer = request
        .channel
        .consumer(Some(target))
        .filter(|c| !c.pending && c.credential.is_some())
        .ok_or(PlanError::ConsumerUnavailable)?;
    if event.consumer.as_ref() != (!target.is_default()).then_some(target)
        || request.consumer != event.consumer
        || event.lineage.is_some()
        || !event.verifies_activity(&request.activity)
        || event.conversation_id != request.conversation_id
        || event.channel_id != request.channel.id
        || event.record_id != request.channel.definition.record_id
    {
        return Err(PlanError::ContextMismatch);
    }
    Ok(RoutedPlan {
        activity: request.activity.clone(),
        source: request
            .channel
            .definition
            .source_of(&request.activity.source)
            .ok_or(PlanError::SourceUnavailable)?
            .clone(),
        targets: vec![ConsumerLaunch {
            consumer: target.clone(),
            provider: consumer.provider.clone(),
            idempotency_key: request.idempotency_key.clone(),
            event,
        }],
        trace: Default::default(),
    })
}
