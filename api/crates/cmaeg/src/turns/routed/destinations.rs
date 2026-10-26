use super::ExecutionError;
pub use crate::delivery::destination::DefaultDestination;
use crate::{channels::resolver::ResolvedChannel, ingress::routing_plan::RoutedPlan};
use cmaeg_core::{channel::ReplyChoice, seal::ReplyAddress};

/// Run before Candidate::prepare for new ingress only. Sync needs an explicit
/// local-stream outcome and is refused before any durable work is admitted.
pub fn validate_routed_delivery(
    channel: &ResolvedChannel,
    plan: &RoutedPlan,
) -> Result<(), ExecutionError> {
    for target in &plan.targets {
        if matches!(target.event.reply, ReplyAddress::Sync { .. }) {
            return Err(ExecutionError::Unsupported);
        }
        let consumer = channel
            .consumer(Some(&target.consumer))
            .ok_or(ExecutionError::AuthorityChanged)?;
        if consumer.reply.destination == ReplyChoice::Default
            && DefaultDestination::capture(channel).is_none()
        {
            return Err(ExecutionError::Unsupported);
        }
    }
    Ok(())
}
