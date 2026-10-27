//! Native cards retain source revision and original dispatch custody for every consumer.
use super::{owner, Card, Error};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, RouteResolution, SourceKind},
    channel::ProviderBinding,
    ids::RecordId,
    routing::{Resolution, RouteInput},
    seal::EventRefPayload,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub(crate) owner: owner::DispatchOwner,
    provider: ProviderBinding,
    source: SourceKind,
    source_revision: String,
    record_id: RecordId,
}
fn source_revision(channel: &ResolvedChannel, source: &SourceKind) -> Result<String, Error> {
    let source = channel.definition.source_of(source).ok_or(Error::Denied)?;
    let value = crate::whatsapp::legacy_source(source);
    Ok(crate::channels::definition::hash_secret(
        &cmaeg_core::profile::canonical_json(value),
    ))
}
pub(crate) fn route(source: &SourceKind, recipient: &str) -> Result<String, Error> {
    let SourceKind::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
    } = source
    else {
        return Err(Error::Denied);
    };
    Ok(format!(
        "whatsapp:{app_id}:{waba_id}:{phone_number_id}:{recipient}"
    ))
}
impl Authority {
    pub(super) fn prepare(
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: &Activity,
    ) -> Result<Self, Error> {
        if !super::whatsapp::eligible(channel, event, activity)
            || !matches!(activity.source, SourceKind::Whatsapp { .. })
        {
            return Err(Error::Denied);
        }
        let consumer = channel
            .consumer(event.consumer.as_ref())
            .ok_or(Error::Denied)?;
        let empty = Default::default();
        let input = RouteInput {
            activity,
            chat_source: channel.definition.source_of(&activity.source),
            card_consumer: Some(event.consumer.as_ref()),
            published: None,
            non_human_providers: &empty,
            attributes_signed: true,
        };
        let Resolution::Routed(decision) =
            cmaeg_core::routing::resolve(&channel.definition, &input, true)
                .map_err(|_| Error::Denied)?
        else {
            return Err(Error::Denied);
        };
        Ok(Self {
            owner: owner::DispatchOwner {
                consumer: event.consumer.clone(),
                conversation_id: event.conversation_id.clone(),
                partition_key: event.partition_key.clone(),
                routing: Some(decision.routing),
            },
            provider: consumer.provider.clone(),
            source: activity.source.clone(),
            source_revision: source_revision(channel, &activity.source)?,
            record_id: event.record_id.clone(),
        })
    }
    pub(super) fn current(
        &self,
        card: &Card,
        channel: &ResolvedChannel,
        activity: &Activity,
    ) -> bool {
        card.owner == channel.definition.owner.subject
            && self.record_id == channel.definition.record_id
            && self.record_id == channel.pointer.record_id
            && self.source == activity.source
            && source_revision(channel, &self.source).ok().as_ref() == Some(&self.source_revision)
            && channel
                .definition
                .secrets
                .whatsapp
                .as_ref()
                .is_some_and(|c| c.validate().is_ok())
            && activity.actor.kind == cmaeg_core::activity::ActorKind::Unknown
            && channel
                .consumer(self.owner.consumer.as_ref())
                .is_some_and(|consumer| {
                    !consumer.pending
                        && consumer.credential.is_some()
                        && consumer.provider == &self.provider
                })
    }
}
pub(super) fn valid(card: &Card) -> bool {
    let Some(authority) = &card.native_whatsapp else {
        return true;
    };
    let owner = &authority.owner;
    let Some(routing) = &owner.routing else {
        return false;
    };
    let target = owner
        .consumer
        .clone()
        .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
    card.native_whatsapp_flow.is_none()
        && card.whatsapp.is_some()
        && card.consumer == owner.consumer
        && !owner.consumer.as_ref().is_some_and(|id| id.is_default())
        && owner.conversation_id.is_wellformed()
        && !owner.partition_key.is_empty()
        && owner.partition_key.len() <= 4096
        && authority.source_revision.starts_with("sha256:")
        && authority.source_revision.len() == 71
        && matches!(&authority.source, SourceKind::Whatsapp { app_id, waba_id, phone_number_id }
            if [app_id, waba_id, phone_number_id].iter().all(|id| cmaeg_core::channel::whatsapp::valid_meta_id(id)))
        && route(&authority.source, &card.actor).ok().as_ref() == Some(&card.route_id)
        && routing.resolution == RouteResolution::Interaction
        && routing.source_id == authority.source.source_id()
        && routing.targets.len() == 1
        && routing.targets[0].id == target
        && routing.rule_id.is_none()
        && routing.causation.is_none()
        && routing.addressing_bytes == 0
        && match (&card.callback_owner, &owner.consumer) {
            (Some(callback), Some(_)) => {
                owner::DispatchOwner::from(callback.clone()) == *owner
                    && callback.provider == authority.provider
                    && callback.source == authority.source
            }
            (None, None) => true,
            _ => false,
        }
}
