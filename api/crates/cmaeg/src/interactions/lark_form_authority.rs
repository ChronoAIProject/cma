//! An issued form retains its provider, source and original conversation.
use super::{lark_form, owner, Card, Error};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::ProviderBinding,
    message::OutboundMessage,
    seal::EventRefPayload,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormAuthority {
    pub(super) block: usize,
    pub(crate) owner: owner::DispatchOwner,
    provider: ProviderBinding,
    source: SourceKind,
    source_revision: String,
}
fn source_revision(channel: &ResolvedChannel, source: &SourceKind) -> Result<String, Error> {
    let source = channel.definition.source_of(source).ok_or(Error::Denied)?;
    let bytes = serde_json::to_value(source).map_err(|_| Error::Denied)?;
    Ok(crate::channels::definition::hash_secret(
        &cmaeg_core::profile::canonical_json(bytes),
    ))
}
impl FormAuthority {
    pub(super) fn prepare(
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: &Activity,
        message: &OutboundMessage,
    ) -> Result<Option<Self>, Error> {
        if !lark_form::requested(message) {
            return Ok(None);
        }
        let (block, _, _) = lark_form::schema(message)?;
        if !owner::eligible(channel, event, activity) {
            return Err(Error::Denied);
        }
        let consumer = channel
            .consumer(event.consumer.as_ref())
            .ok_or(Error::Denied)?;
        let owner = owner::prepare(channel, event, activity)?
            .map(Into::into)
            .unwrap_or_else(|| owner::DispatchOwner {
                consumer: None,
                conversation_id: event.conversation_id.clone(),
                partition_key: event.partition_key.clone(),
                routing: None,
            });
        Ok(Some(Self {
            block,
            owner,
            provider: consumer.provider.clone(),
            source: activity.source.clone(),
            source_revision: source_revision(channel, &activity.source)?,
        }))
    }
    pub(super) fn current(
        &self,
        card: &Card,
        channel: &ResolvedChannel,
        activity: &Activity,
    ) -> bool {
        self.source == activity.source && self.current_source(card, channel)
    }
    pub(super) fn current_source(&self, card: &Card, channel: &ResolvedChannel) -> bool {
        card.owner == channel.definition.owner.subject
            && source_revision(channel, &self.source).ok().as_ref() == Some(&self.source_revision)
            && channel
                .consumer(self.owner.consumer.as_ref())
                .is_some_and(|consumer| {
                    !consumer.pending
                        && consumer.credential.is_some()
                        && consumer.provider == &self.provider
                })
    }
    pub(super) fn replacement(&self, message: &OutboundMessage) -> Result<Self, Error> {
        let mut next = self.clone();
        next.block = lark_form::schema(message)?.0;
        Ok(next)
    }
}
pub(super) fn valid(card: &Card) -> bool {
    let form = card
        .lark
        .as_ref()
        .and_then(|identity| identity.form.as_ref());
    let Some(form) = form else {
        return card
            .consumed
            .as_ref()
            .is_none_or(|click| click.submitted.is_none());
    };
    let Ok((index, _, fields)) = lark_form::schema(&card.message) else {
        return false;
    };
    form.block == index
        && card.consumer == form.owner.consumer
        && !form
            .owner
            .consumer
            .as_ref()
            .is_some_and(|id| id.is_default())
        && !form.owner.partition_key.is_empty()
        && form.owner.partition_key.len() <= 4096
        && form.owner.conversation_id.is_wellformed()
        && !form.source_revision.is_empty()
        && super::lark::render_form(&card.message, &card.id, card.revision).is_ok()
        && matches!(&form.source, SourceKind::NyxidRelay {platform,route_id}
            if Some(*platform) == card.lark.as_ref().map(|identity| identity.platform) && route_id == &card.route_id)
        && match (&card.callback_owner, &form.owner.consumer) {
            (Some(owner), Some(_)) => owner::DispatchOwner::from(owner.clone()) == form.owner,
            (None, None) => form.owner.routing.is_none(),
            _ => false,
        }
        && card.consumed.as_ref().is_none_or(|click| {
            click.action == index
                && click.selection.is_none()
                && click
                    .submitted
                    .as_ref()
                    .is_some_and(|values| lark_form::valid_values(fields, values))
        })
}
