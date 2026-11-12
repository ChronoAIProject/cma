//! One CAS persists the canonical winning click and everything local recovery needs.
use super::super::{Card, Click, InteractionStore};
use super::{authority::hash, *};
use crate::{channels::resolver::ResolvedChannel, native_lark::callback::Callback};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, InteractionResult,
        SourceKind,
    },
    ids::EventId,
    seal::{event_ref::activity_digest, EventRef, EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pending {
    pub callback: Callback,
    pub activity: Activity,
    pub event: EventRefPayload,
    pub reference: EventRef,
    pub admitted: bool,
}
impl InteractionStore {
    /// The ingress caller must hold the shared native exact-message control fence.
    /// The card CAS chooses one winner; the fence orders consumption against edits/effects.
    pub(crate) async fn consume_native_lark(
        &self,
        channel: &ResolvedChannel,
        callback: &Callback,
    ) -> Result<String, Error> {
        let (id, index, revision) = super::decode(&callback.locator)?;
        for _ in 0..2 {
            let (mut card, version) = self.load(&id).await?;
            self.native_lark_current(&card, channel).await?;
            let a = card.native_lark.as_ref().ok_or(Error::Denied)?;
            let destination = a.destination.as_ref().ok_or(Error::Unavailable)?;
            if card.superseded
                || card.reference.is_none()
                || revision != Some(card.revision)
                || callback.locator.starts_with("cnlf1:") != a.form
                || callback.identity != a.reply().identity
                || callback.actor != card.actor
                || callback.chat != card.chat
                || callback.message != destination.message_id
                || callback.occurred_at < a.issued_at
                || callback.occurred_at > super::super::now() + 300
                || callback.occurred_at >= card.expires_at
            {
                return Err(Error::Denied);
            }
            if let Some(pending) = &a.pending {
                return if pending.callback == *callback {
                    Ok(id)
                } else {
                    Err(Error::Denied)
                };
            }
            let source = channel
                .definition
                .source_of(&SourceKind::NativeLark(callback.identity.clone()))
                .ok_or(Error::Denied)?;
            let cmaeg_core::channel::Source::NativeLark(source) = source else {
                return Err(Error::Denied);
            };
            let conversation = ActivityConversation {
                id: card.chat.clone(),
                kind: a.conversation,
                thread_id: card.thread.clone(),
            };
            crate::native_lark::message::admission(
                source,
                &card.actor,
                &conversation,
                a.reply().mentioned,
            )
            .map_err(|_| Error::Denied)?;
            let (action, submitted) = if a.form {
                let submitted = super::super::lark_form::submitted(
                    &card.message,
                    index,
                    callback.form.as_ref().ok_or(Error::Denied)?,
                )?;
                let (_, name, _) = super::super::lark_form::schema(&card.message)?;
                (
                    super::super::card::Action {
                        block: name.into(),
                        id: name.into(),
                        selected: vec![],
                    },
                    Some(submitted),
                )
            } else {
                if callback.form.is_some() {
                    return Err(Error::Denied);
                }
                (
                    super::super::keyboard::actions(&card.message)
                        .get(index)
                        .cloned()
                        .ok_or(Error::Denied)?,
                    None,
                )
            };
            let event_id = EventId::new(format!(
                "native-lark-control:{}",
                hash(&(callback.identity.clone(), &callback.event_id))
            ))
            .map_err(|_| Error::Denied)?;
            let occurred_at = OffsetDateTime::from_unix_timestamp(callback.occurred_at)
                .map_err(|_| Error::Denied)?
                .format(&Rfc3339)
                .map_err(|_| Error::Denied)?;
            let mut values = json!({"message_ref":card.reference,"interaction_record_id":card.id,"revision":card.revision,"platform_message_id":destination.message_id});
            if let Some(submitted) = &submitted {
                values["fields"] = json!(submitted);
            } else {
                values["selected"] = json!(action.selected);
            }
            let activity = Activity {
                schema_version: 1,
                event_id: event_id.clone(),
                thread: None,
                actor: Actor {
                    id: card.actor.clone(),
                    kind: ActorKind::Human,
                    display_name: None,
                },
                conversation,
                kind: ActivityKind::Interaction(InteractionResult {
                    block_id: action.block,
                    action_id: action.id,
                    values: values.as_object().unwrap().clone(),
                }),
                occurred_at: occurred_at.clone(),
                source: SourceKind::NativeLark(callback.identity.clone()),
                routing: a.owner.routing.clone(),
            };
            activity.validate().map_err(|_| Error::Denied)?;
            let event = EventRefPayload {
                v: 1,
                channel_id: card.channel_id.clone(),
                record_id: a.original.record_id.clone(),
                event_id: event_id.clone(),
                partition_key: a.owner.partition_key.clone(),
                conversation_id: a.owner.conversation_id.clone(),
                expires_at: card.expires_at,
                reply: ReplyAddress::NativeLark(destination.reply.clone()),
                consumer: card.consumer.clone(),
                lineage: None,
                telegram_admission: None,
                turn_trace: None,
                activity_sha256: Some(activity_digest(&activity)),
            };
            let reference = EventRef::seal(&event, &self.keys).map_err(|_| Error::Unavailable)?;
            card.consumed = Some(Click {
                event_id: event_id.to_string(),
                callback_id: callback.event_id.clone(),
                action: index,
                selection: None,
                submitted,
                form_launch: None,
                occurred_at,
            });
            card.native_lark.as_mut().unwrap().pending = Some(Pending {
                callback: callback.clone(),
                activity,
                event,
                reference,
                admitted: false,
            });
            match self.save(&card, Some(version)).await {
                Ok(_) => return Ok(id),
                Err(Error::Denied) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Unavailable)
    }
    pub(crate) async fn native_lark_pending(
        &self,
        id: &str,
        channel: &ResolvedChannel,
    ) -> Result<Pending, Error> {
        let (card, _) = self.load(id).await?;
        self.native_lark_current(&card, channel).await?;
        let a = card.native_lark.as_ref().ok_or(Error::Denied)?;
        let pending = a.pending.as_ref().ok_or(Error::Denied)?;
        if card.superseded
            || !valid_pending(&card, pending)
            || !pending
                .reference
                .open(&self.keys, super::super::now())
                .is_ok_and(|r| r.payload == pending.event)
        {
            return Err(Error::Denied);
        }
        let cmaeg_core::channel::Source::NativeLark(source) = channel
            .definition
            .source_of(&pending.activity.source)
            .ok_or(Error::Denied)?
        else {
            return Err(Error::Denied);
        };
        crate::native_lark::message::admission(
            source,
            &card.actor,
            &pending.activity.conversation,
            a.reply().mentioned,
        )
        .map_err(|_| Error::Denied)?;
        Ok(pending.clone())
    }
    pub(crate) async fn native_lark_admitted(&self, id: &str) -> Result<(), Error> {
        for _ in 0..2 {
            let (mut card, version) = self.load(id).await?;
            if card.superseded {
                return Err(Error::Denied);
            }
            let pending = card
                .native_lark
                .as_mut()
                .and_then(|a| a.pending.as_mut())
                .ok_or(Error::Denied)?;
            if pending.admitted {
                return Ok(());
            }
            pending.admitted = true;
            match self.save(&card, Some(version)).await {
                Ok(_) => return Ok(()),
                Err(Error::Denied) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Unavailable)
    }
}
pub(super) fn valid_pending(card: &Card, pending: &Pending) -> bool {
    let Some(click) = &card.consumed else {
        return false;
    };
    let Some(a) = &card.native_lark else {
        return false;
    };
    let Ok((id, index, revision)) = super::decode(&pending.callback.locator) else {
        return false;
    };
    let ActivityKind::Interaction(result) = &pending.activity.kind else {
        return false;
    };
    let (action, submitted) = if a.form {
        let Some(form) = &pending.callback.form else {
            return false;
        };
        let Ok(values) = super::super::lark_form::submitted(&card.message, index, form) else {
            return false;
        };
        let Ok((_, name, _)) = super::super::lark_form::schema(&card.message) else {
            return false;
        };
        (
            super::super::card::Action {
                block: name.into(),
                id: name.into(),
                selected: vec![],
            },
            Some(values),
        )
    } else {
        if pending.callback.form.is_some() {
            return false;
        }
        let Some(action) = super::super::keyboard::actions(&card.message)
            .get(index)
            .cloned()
        else {
            return false;
        };
        (action, None)
    };
    let Some(destination) = &a.destination else {
        return false;
    };
    let mut values = json!({"message_ref":card.reference,"interaction_record_id":card.id,"revision":card.revision,"platform_message_id":destination.message_id});
    if let Some(submitted) = &submitted {
        values["fields"] = json!(submitted);
    } else {
        values["selected"] = json!(action.selected);
    }
    let expected_time = OffsetDateTime::from_unix_timestamp(pending.callback.occurred_at)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok());
    id == card.id
        && revision == Some(card.revision)
        && index == click.action
        && pending.callback.locator.starts_with("cnlf1:") == a.form
        && click.selection.is_none()
        && click.form_launch.is_none()
        && click.submitted == submitted
        && result.block_id == action.block
        && result.action_id == action.id
        && Some(&result.values) == values.as_object()
        && expected_time.as_ref() == Some(&click.occurred_at)
        && pending.callback.occurred_at >= a.issued_at
        && pending.callback.occurred_at < card.expires_at
        && pending.activity.thread.is_none()
        && pending.activity.source == SourceKind::NativeLark(a.reply().identity.clone())
        && pending.event.event_id.as_str()
            == format!(
                "native-lark-control:{}",
                hash(&(
                    pending.callback.identity.clone(),
                    &pending.callback.event_id
                ))
            )
        && pending.activity.validate().is_ok()
        && pending.event.validate().is_ok()
        && pending.event.verifies_activity(&pending.activity)
        && pending.activity.event_id.as_str() == click.event_id
        && pending.callback.event_id == click.callback_id
        && pending.activity.actor.id == card.actor
        && pending.activity.actor.kind == ActorKind::Human
        && pending.activity.conversation.id == card.chat
        && pending.activity.conversation.thread_id == card.thread
        && pending.activity.conversation.kind == a.conversation
        && pending.activity.routing == a.owner.routing
        && pending.event.consumer == card.consumer
        && pending.event.channel_id == card.channel_id
        && pending.event.record_id == a.original.record_id
        && pending.event.partition_key == a.owner.partition_key
        && pending.event.conversation_id == a.owner.conversation_id
        && pending.event.expires_at == card.expires_at
        && pending.activity.occurred_at == click.occurred_at
        && pending.callback.identity == a.reply().identity
        && pending.callback.actor == card.actor
        && pending.callback.chat == card.chat
        && a.destination.as_ref().is_some_and(|r| {
            pending.callback.message == r.message_id
                && pending.event.reply == ReplyAddress::NativeLark(r.reply.clone())
        })
}
