//! Dispatch reopens the winning card instead of trusting mutable current routing.
use super::super::{Card, Error};
use crate::{channels::resolver::ResolvedChannel, records::RecordClient};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    seal::{open_consumer_interaction, open_interaction, EventRefPayload, MasterKeyRing},
};
use std::sync::Arc;
pub(crate) async fn current(
    records: &RecordClient,
    keys: &Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    activity: &Activity,
    event: &EventRefPayload,
) -> Result<(), Error> {
    let ActivityKind::Interaction(result) = &activity.kind else {
        return Err(Error::Denied);
    };
    let id = result
        .values
        .get("interaction_record_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Denied)?;
    let records = records.interactions();
    let url = records.record_url(id).map_err(|_| Error::Denied)?;
    let row = records.get(url.as_str()).await.map_err(|e| match e {
        crate::records::RecordError::NotFound => Error::Denied,
        _ => Error::Unavailable,
    })?;
    let opened = if event.consumer.is_some() {
        open_consumer_interaction(keys, id, &row.blob)
    } else {
        open_interaction(keys, id, &row.blob)
    }
    .map_err(|_| Error::Denied)?;
    let card: Card = serde_json::from_slice(&opened.payload).map_err(|_| Error::Denied)?;
    let a = card.native_lark.as_ref().ok_or(Error::Denied)?;
    let pending = a.pending.as_ref().ok_or(Error::Denied)?;
    if card.id != id
        || !a.current(&card, channel)
        || !super::store::valid_reference(&card, keys)
        || card.consumer != event.consumer
        || (card.superseded && !pending.admitted)
        || card.expires_at <= super::super::now()
        || pending.activity != *activity
        || pending.event != *event
        || !pending
            .reference
            .open(keys, super::super::now())
            .is_ok_and(|r| r.payload == *event)
    {
        return Err(Error::Denied);
    }
    let destination = a.destination.as_ref().ok_or(Error::Denied)?;
    let receipt = crate::native_lark::receipts::Store::new(records, keys.clone())
        .load(
            &card.channel_id,
            &destination.source,
            &destination.message_id,
            super::super::now(),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Denied)?;
    if receipt != *destination {
        return Err(Error::Denied);
    }
    let cmaeg_core::channel::Source::NativeLark(source) = channel
        .definition
        .source_of(&activity.source)
        .ok_or(Error::Denied)?
    else {
        return Err(Error::Denied);
    };
    crate::native_lark::message::admission(
        source,
        &card.actor,
        &activity.conversation,
        a.reply().mentioned,
    )
    .map_err(|_| Error::Denied)
}
