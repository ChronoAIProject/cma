//! Dispatch reopens the exact winning card, native receipt and current source custody.
use super::{click::valid_pending, store::valid_reference, Error};
use crate::{channels::resolver::ResolvedChannel, interactions::Card, records::RecordClient};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    seal::{open_consumer_interaction, open_interaction, EventRefPayload, MasterKeyRing},
};
use std::sync::Arc;
pub(crate) fn shape(activity: &Activity) -> bool {
    activity
        .event_id
        .as_str()
        .starts_with("native-whatsapp-flow:")
        && matches!(activity.kind, ActivityKind::Interaction(_))
        && matches!(
            activity.source,
            cmaeg_core::activity::SourceKind::Whatsapp { .. }
        )
}
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
    let cards = records.clone().whatsapp_flows();
    let url = cards.record_url(id).map_err(|_| Error::Denied)?;
    let row = cards.get(url.as_str()).await.map_err(|e| match e {
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
    let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Denied)?;
    let pending = a.pending.as_ref().ok_or(Error::Denied)?;
    if row.version == 0
        || card.consumer.as_ref().is_some_and(|c| c.is_default())
        || card.consumer != event.consumer
        || card.id != id
        || card.expires_at <= super::super::now()
        || card.superseded
        || !a.current(&card, channel)
        || !valid_reference(&card, keys)
        || !valid_pending(&card, pending)
        || super::click::completion_activity(&card, &pending.callback).as_ref() != Some(activity)
        || pending.event != *event
        || !pending
            .reference
            .open(keys, super::super::now())
            .is_ok_and(|r| r.payload == *event)
    {
        return Err(Error::Denied);
    }
    let receipt = a.receipt().ok_or(Error::Denied)?;
    let stored = crate::whatsapp::receipts::Store::new(records.clone(), keys.clone())
        .load(
            &card.channel_id,
            &receipt.source,
            &receipt.recipient,
            &receipt.message_id,
            super::super::now(),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Denied)?;
    if stored != *receipt {
        return Err(Error::Denied);
    }
    Ok(())
}
