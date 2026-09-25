//! Project immutable CMA configuration into the Event Gateway wire contract.
use layer2_core::{
    bots::{Bot, BotError},
    event_workers::projection::{self, WorkerProjection},
    ServiceId,
};
use serde_json::Value;
fn projection(bot: &Bot) -> WorkerProjection<'_> {
    WorkerProjection {
        name: &bot.request.name,
        owner: &bot.owner,
        provider: &bot.provider,
        profiles: &bot.profiles,
    }
}
pub(super) fn services(bot: &Bot) -> Result<Vec<ServiceId>, BotError> {
    projection::services(&projection(bot))
}
pub(super) fn snapshot(bot: &Bot) -> Result<Value, BotError> {
    if let Some(pin) = &bot.profiles.event_worker_profile {
        return Ok(pin.snapshot.clone());
    }
    projection::snapshot_for_owner(&projection(bot), &bot.resource_owner())
}
