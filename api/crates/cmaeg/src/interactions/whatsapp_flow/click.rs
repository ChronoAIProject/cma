//! One winning CAS retains the exact typed completion and durable route outbox.
use super::{Error, Pending, Phase};
use crate::{
    channels::resolver::ResolvedChannel,
    interactions::{Card, Click, InteractionStore},
    whatsapp::forms::{callback::Callback, compile, values},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, InteractionResult},
    seal::{event_ref::activity_digest, EventRef, EventRefPayload, ReplyAddress},
};

impl InteractionStore {
    pub(crate) async fn consume_whatsapp_flow(
        &self,
        channel: &ResolvedChannel,
        callback: &Callback,
    ) -> Result<String, Error> {
        if !callback.valid() {
            return Err(Error::Denied);
        }
        for _ in 0..2 {
            let (mut card, version) = self.load_whatsapp_flow(&callback.card).await?;
            self.whatsapp_flow_current(&card, channel).await?;
            let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Denied)?;
            let Phase::Bound { receipt, .. } = &a.phase else {
                return Err(Error::Unavailable);
            };
            if callback.revision != card.revision
                || callback.source != a.activity.source
                || callback.sender != card.actor
                || callback.context != receipt.message_id
                || callback.timestamp < a.issued_at
                || callback.timestamp >= card.expires_at
                || callback.timestamp > super::super::now() + 300
            {
                return Err(Error::Denied);
            }
            if let Some(pending) = &a.pending {
                return if pending.callback == *callback {
                    Ok(card.id)
                } else {
                    Err(Error::Denied)
                };
            }
            let activity = completion_activity(&card, callback).ok_or(Error::Denied)?;
            let (index, _, _) = compile::schema(&card.message).map_err(|_| Error::Denied)?;
            let event_id = activity.event_id.clone();
            let occurred_at = activity.occurred_at.clone();
            let mut reply = a.original.reply.clone();
            let ReplyAddress::Whatsapp { message_id, .. } = &mut reply else {
                return Err(Error::Denied);
            };
            *message_id = callback.id.clone();
            let event = EventRefPayload {
                v: 1,
                channel_id: card.channel_id.clone(),
                record_id: a.original.record_id.clone(),
                event_id: event_id.clone(),
                partition_key: a.owner.partition_key.clone(),
                conversation_id: a.owner.conversation_id.clone(),
                expires_at: card.expires_at,
                reply,
                consumer: card.consumer.clone(),
                lineage: None,
                telegram_admission: None,
                turn_trace: None,
                activity_sha256: Some(activity_digest(&activity)),
            };
            let reference = EventRef::seal(&event, &self.keys).map_err(|_| Error::Unavailable)?;
            card.consumed = Some(Click {
                event_id: event_id.to_string(),
                callback_id: callback.id.clone(),
                action: index,
                selection: None,
                submitted: None,
                form_launch: None,
                occurred_at,
            });
            card.native_whatsapp_flow.as_mut().unwrap().pending = Some(Pending {
                event,
                reference,
                callback: callback.clone(),
                admitted: false,
            });
            match self.save_whatsapp_flow(&card, version).await {
                Ok(_) => return Ok(card.id),
                Err(Error::Denied) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Unavailable)
    }
    pub(crate) async fn whatsapp_flow_pending(
        &self,
        id: &str,
        channel: &ResolvedChannel,
    ) -> Result<super::Completion, Error> {
        let (card, _) = self.load_whatsapp_flow(id).await?;
        self.whatsapp_flow_current(&card, channel).await?;
        let pending = card
            .native_whatsapp_flow
            .as_ref()
            .and_then(|a| a.pending.as_ref())
            .ok_or(Error::Denied)?;
        if !valid_pending(&card, pending)
            || !pending
                .reference
                .open(&self.keys, super::super::now())
                .is_ok_and(|r| r.payload == pending.event)
        {
            return Err(Error::Denied);
        }
        Ok(super::Completion {
            activity: completion_activity(&card, &pending.callback).ok_or(Error::Denied)?,
            event: pending.event.clone(),
            reference: pending.reference.clone(),
            callback: pending.callback.clone(),
            admitted: pending.admitted,
        })
    }
    pub(crate) async fn whatsapp_flow_admitted(&self, id: &str) -> Result<(), Error> {
        for _ in 0..2 {
            let (mut card, version) = self.load_whatsapp_flow(id).await?;
            let pending = card
                .native_whatsapp_flow
                .as_mut()
                .and_then(|a| a.pending.as_mut())
                .ok_or(Error::Denied)?;
            if pending.admitted {
                return Ok(());
            }
            pending.admitted = true;
            match self.save_whatsapp_flow(&card, version).await {
                Ok(_) => return Ok(()),
                Err(Error::Denied) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Unavailable)
    }
}
pub(super) fn valid_pending(card: &Card, pending: &Pending) -> bool {
    let Some(a) = &card.native_whatsapp_flow else {
        return false;
    };
    let Some(receipt) = a.receipt() else {
        return false;
    };
    let Ok((index, _, _)) = compile::schema(&card.message) else {
        return false;
    };
    let Some(activity) = completion_activity(card, &pending.callback) else {
        return false;
    };
    let mut expected_reply = a.original.reply.clone();
    if let ReplyAddress::Whatsapp { message_id, .. } = &mut expected_reply {
        *message_id = pending.callback.id.clone();
    } else {
        return false;
    }
    pending.callback.valid()
        && matches!(a.phase, Phase::Bound { .. })
        && pending.callback.timestamp >= a.issued_at
        && pending.callback.timestamp < card.expires_at
        && activity.thread.is_none()
        && activity.schema_version == 1
        && activity.validate().is_ok()
        && pending.callback.card == card.id
        && pending.callback.revision == card.revision
        && pending.callback.source == a.activity.source
        && pending.callback.sender == card.actor
        && pending.callback.context == receipt.message_id
        && pending.callback.event_id().ok().as_ref() == Some(&activity.event_id)
        && activity.actor == a.activity.actor
        && activity.source == a.activity.source
        && activity.conversation == a.activity.conversation
        && activity.routing == a.owner.routing
        && pending.event.reply == expected_reply
        && pending.event.lineage.is_none()
        && pending.event.telegram_admission.is_none()
        && pending.event.verifies_activity(&activity)
        && pending.event.consumer == card.consumer
        && pending.event.channel_id == card.channel_id
        && pending.event.record_id == a.original.record_id
        && pending.event.expires_at == card.expires_at
        && pending.event.conversation_id == a.owner.conversation_id
        && pending.event.partition_key == a.owner.partition_key
        && card.consumed.as_ref().is_some_and(|click| {
            click.occurred_at == activity.occurred_at
                && click.selection.is_none()
                && click.form_launch.is_none()
                && click.action == index
                && click.submitted.is_none()
                && click.callback_id == pending.callback.id
                && click.event_id == activity.event_id.as_str()
        })
}

// Only the bounded signed callback is persisted. Typed values and activity are
// deterministically reconstructed and checked against the sealed EventRef digest.
pub(super) fn completion_activity(card: &Card, callback: &Callback) -> Option<Activity> {
    let a = card.native_whatsapp_flow.as_ref()?;
    let receipt = a.receipt()?;
    let fields = values::submitted(&card.message, &callback.fields).ok()?;
    let (_, form_id, _) = compile::schema(&card.message).ok()?;
    let occurred_at = time::OffsetDateTime::from_unix_timestamp(callback.timestamp)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()?;
    let activity = Activity {
        schema_version: 1,
        thread: None,
        event_id: callback.event_id().ok()?,
        actor: a.activity.actor.clone(),
        conversation: a.activity.conversation.clone(),
        source: a.activity.source.clone(),
        occurred_at,
        routing: a.owner.routing.clone(),
        kind: ActivityKind::Interaction(InteractionResult {
            block_id: form_id.into(),
            action_id: form_id.into(),
            values: serde_json::json!({"fields": fields, "message_ref": card.reference,
                "interaction_record_id": card.id, "revision": card.revision,
                "platform_message_id": receipt.message_id})
            .as_object()?
            .clone(),
        }),
    };
    activity.validate().ok()?;
    Some(activity)
}
