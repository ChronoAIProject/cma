use super::ExecutionError;
pub use crate::delivery::destination::DefaultDestination;
use crate::{channels::resolver::ResolvedChannel, ingress::routing_plan::RoutedPlan};
use cmaeg_core::{
    activity::SourceKind,
    channel::{ReplyChoice, Sink},
    seal::ReplyAddress,
};

/// Default remains closed for process-local Sync ingress.
pub fn validate_routed_delivery(
    channel: &ResolvedChannel,
    plan: &RoutedPlan,
) -> Result<(), ExecutionError> {
    validate_routed_delivery_with_sync(channel, plan, false)
}

/// Run before new admission, opting in only with the same local waiter registry
/// wired into DeliveryCoordinator::with_custom_stream. Replay is not admission.
pub fn validate_routed_delivery_with_sync(
    channel: &ResolvedChannel,
    plan: &RoutedPlan,
    local_stream_enabled: bool,
) -> Result<(), ExecutionError> {
    for target in &plan.targets {
        let consumer = channel
            .consumer(Some(&target.consumer))
            .ok_or(ExecutionError::AuthorityChanged)?;
        if matches!(target.event.reply, ReplyAddress::Sync { .. })
            && (!local_stream_enabled
                || plan.activity.source != SourceKind::Custom
                || consumer.reply.destination != ReplyChoice::Origin
                || !channel
                    .definition
                    .reply
                    .sinks
                    .iter()
                    .any(|sink| matches!(sink, Sink::Sync {})))
        {
            return Err(ExecutionError::Unsupported);
        }
        if consumer.reply.destination == ReplyChoice::Default
            && DefaultDestination::capture(channel).is_none()
        {
            return Err(ExecutionError::Unsupported);
        }
    }
    Ok(())
}
