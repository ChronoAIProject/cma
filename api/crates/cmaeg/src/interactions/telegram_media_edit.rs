//! A media edit advances the original card chain under its existing CAS.
use super::{Card, Error, InteractionStore, PreparedCard};
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::{
        intents::{DeliveryOutcome, IntentReceipt},
        render::{RelayText, Renderer},
    },
    native_media::model::{Presentation, Snapshot},
    tools::message_effects::mutation,
};
use cmaeg_core::{
    message::{Block, OutboundMessage},
    seal::MessageRefPayload,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edit {
    request_hash: String,
    successor: String,
    receipt: Option<IntentReceipt>,
    lock: mutation::RetainedLock,
}
pub(crate) struct Candidate {
    pub card: Card,
    pub channel: Arc<ResolvedChannel>,
    pub caption: String,
    version: u64,
}
pub(crate) enum Admission {
    Ready(Box<Candidate>),
    Replay(IntentReceipt),
}
pub(crate) struct Claim {
    previous: Card,
    version: u64,
    pub prepared: PreparedCard,
}
pub(super) fn valid(card: &Card) -> bool {
    let Some(a) = &card.native_telegram_media else {
        return true;
    };
    let Some(snapshot) = &a.snapshot else {
        return a.edit.is_none() && card.revision == 1;
    };
    snapshot.kind == a.kind
        && snapshot.caption == a.caption
        && snapshot.content.is_empty()
        && !snapshot.files.is_empty()
        && snapshot.files.len() <= 24
        && snapshot
            .files
            .iter()
            .all(|f| !f.id.is_empty() && f.id.len() <= 512)
        && matches!(snapshot.presentation, Some(Presentation::Telegram { .. }))
        && serde_json::to_vec(snapshot).is_ok_and(|b| b.len() <= 16384)
        && a.edit.as_ref().is_none_or(|edit| {
            card.superseded
                && edit.request_hash.len() == 64
                && edit.request_hash.bytes().all(|b| b.is_ascii_hexdigit())
                && uuid::Uuid::parse_str(&edit.successor).is_ok_and(|id| id.get_version_num() == 4)
                && edit.successor != card.id
                && card.destination.as_ref().is_some_and(|d| {
                    edit.lock.telegram_media(
                        &mutation::destination(
                            card.route_id
                                .strip_prefix("telegram:")
                                .unwrap_or("")
                                .split(':')
                                .next()
                                .unwrap_or(""),
                            &card.chat,
                            &d.platform_id,
                        ),
                        &card.owner,
                        card.expires_at,
                    )
                })
        })
}
pub(super) fn callback_replay(card: &Card, event: &str, callback: &str, action: usize) -> bool {
    card.native_telegram_media
        .as_ref()
        .is_some_and(|a| a.edit.is_some())
        && card.consumed.as_ref().is_some_and(|c| {
            c.event_id == event
                && c.callback_id == callback
                && c.action == action
                && c.submitted.is_none()
        })
}
pub(crate) fn caption(message: &OutboundMessage, previous: &str) -> Result<String, Error> {
    if message.validate().is_err()
        || !message.native_attachments.is_empty()
        || message.native_media_update.is_some()
        || message.whatsapp_template.is_some()
        || message.reply_to.is_some()
        || message.thread_reply
    {
        return Err(Error::Denied);
    }
    let mut text: Vec<String> = message.text.iter().cloned().collect();
    for block in &message.blocks {
        match block {
            Block::Text { text: part } => text.push(part.clone()),
            Block::Choice { prompt, .. } => text.push(prompt.clone()),
            Block::Buttons { .. } | Block::Link { .. } => {}
            _ => return Err(Error::Denied),
        }
    }
    let caption = if text.is_empty() {
        previous.to_owned()
    } else {
        text.join("\n\n")
    };
    if caption.encode_utf16().count() > 1024
        || caption.chars().any(|c| {
            !matches!(c, '\n' | '\r' | '\t') && cmaeg_core::activity::forbidden_text_char(c)
        })
    {
        return Err(Error::Denied);
    }
    Ok(caption)
}
pub(crate) fn receipt(message: &OutboundMessage, delivery: DeliveryOutcome) -> IntentReceipt {
    let (_, rendering) = RelayText.render(message, &crate::tools::Capabilities::native_telegram());
    IntentReceipt {
        message_ref: None,
        rendering,
        delivery,
    }
}
pub(crate) fn uncertain(message: &OutboundMessage) -> IntentReceipt {
    receipt(
        message,
        DeliveryOutcome::Unknown {
            reason: "telegram_media_card_edit_in_progress_or_uncertain".into(),
        },
    )
}
impl Candidate {
    pub fn chat(&self) -> &str {
        &self.card.chat
    }
    pub fn thread(&self) -> Option<&str> {
        self.card.thread.as_deref()
    }
    pub fn bot(&self) -> &str {
        self.card
            .route_id
            .strip_prefix("telegram:")
            .unwrap()
            .split(':')
            .next()
            .unwrap()
    }
    pub fn destination(&self) -> String {
        mutation::destination(
            self.bot(),
            &self.card.chat,
            &self.card.destination.as_ref().unwrap().platform_id,
        )
    }
    pub fn snapshot(&self) -> &Snapshot {
        self.card
            .native_telegram_media
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
    }
}
impl InteractionStore {
    pub(crate) fn telegram_media_mutations(&self) -> mutation::Store {
        mutation::Store::telegram_media(self.records.clone(), self.keys.clone())
    }
    pub(crate) async fn telegram_media_edit_candidate(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> Result<Admission, Error> {
        let (card, version) = self
            .load(
                reference
                    .interaction_record_id
                    .as_deref()
                    .ok_or(Error::Denied)?,
            )
            .await?;
        let a = card.native_telegram_media.as_ref().ok_or(Error::Denied)?;
        let saved = card
            .reference
            .as_ref()
            .ok_or(Error::Denied)?
            .open(&self.keys)
            .map_err(|_| Error::Denied)?
            .payload;
        let destination = card.destination.as_ref().ok_or(Error::Denied)?;
        if &saved != reference
            || reference.validate().is_err()
            || reference.channel_id != card.channel_id
            || reference.event_id != card.event_id
            || reference.consumer != card.consumer
            || reference.route_id != card.route_id
            || card.channel_id != channel.id
            || card.owner != channel.definition.owner.subject
            || a.snapshot.is_none()
            || destination.outbound_id != destination.platform_id
            || destination.platform_id != reference.platform_message_id
        {
            return Err(Error::Denied);
        }
        let current = self.telegram_media_channel(&card).await?;
        let requested_caption = caption(message, &a.caption)?;
        if let Some(edit) = &a.edit {
            if edit.request_hash != mutation::hash(message) {
                return Err(Error::Denied);
            }
            if let Some(receipt) = &edit.receipt {
                self.telegram_edit_receipt(&card, &edit.successor, receipt)?;
                if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) {
                    let _ = self
                        .telegram_media_mutations()
                        .release_retained(&edit.lock)
                        .await;
                    return Ok(Admission::Replay(receipt.clone()));
                }
            }
            // Binding is proof of a verified platform response. Recover that
            // exact successor when the final previous-card acknowledgement was lost.
            if let Ok((next, _)) = self.load(&edit.successor).await {
                if let Some(next_a) = &next.native_telegram_media {
                    if next.reference.is_some()
                        && next.owner == card.owner
                        && next.channel_id == card.channel_id
                        && next.event_id == card.event_id
                        && next.route_id == card.route_id
                        && next.actor == card.actor
                        && next.chat == card.chat
                        && next.thread == card.thread
                        && next.consumer == card.consumer
                        && next.callback_owner == card.callback_owner
                        && Some(next.revision) == card.revision.checked_add(1)
                        && a.same_custody(next_a)
                        && next_a.caption == requested_caption
                        && mutation::hash(&next.message) == edit.request_hash
                        && next.expires_at == card.expires_at
                        && next_a
                            .snapshot
                            .as_ref()
                            .is_some_and(|s| a.snapshot.as_ref().unwrap().same_files(s))
                    {
                        let mut result = receipt(
                            message,
                            DeliveryOutcome::Confirmed {
                                platform_message_id: Some(destination.platform_id.clone()),
                            },
                        );
                        result.message_ref = next.reference;
                        self.telegram_edit_receipt(&card, &edit.successor, &result)?;
                        let _ = self
                            .telegram_media_mutations()
                            .release_retained(&edit.lock)
                            .await;
                        return Ok(Admission::Replay(result));
                    }
                }
            }
            return Ok(Admission::Replay(uncertain(message)));
        }
        if card.superseded {
            return Err(Error::Denied);
        }
        Ok(Admission::Ready(Box::new(Candidate {
            card,
            channel: current,
            caption: requested_caption,
            version,
        })))
    }
    fn telegram_edit_receipt(
        &self,
        previous: &Card,
        successor: &str,
        receipt: &IntentReceipt,
    ) -> Result<(), Error> {
        if let Some(reference) = &receipt.message_ref {
            let next = reference
                .open(&self.keys)
                .map_err(|_| Error::Denied)?
                .payload;
            if next.interaction_record_id.as_deref() != Some(successor)
                || next.channel_id != previous.channel_id
                || next.event_id != previous.event_id
                || next.consumer != previous.consumer
                || next.route_id != previous.route_id
                || previous
                    .destination
                    .as_ref()
                    .is_none_or(|d| d.platform_id != next.platform_message_id)
                || !matches!(&receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id: Some(id) } if id == &next.platform_message_id)
            {
                return Err(Error::Denied);
            }
        } else if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
            return Err(Error::Denied);
        }
        Ok(())
    }
    pub(crate) async fn begin_telegram_media_edit(
        &self,
        candidate: Candidate,
        message: &OutboundMessage,
        lock: mutation::RetainedLock,
    ) -> Result<Claim, Error> {
        let mut previous = candidate.card;
        let mut next = previous.clone();
        next.id = uuid::Uuid::new_v4().to_string();
        next.revision = next.revision.checked_add(1).ok_or(Error::Denied)?;
        next.reference = None;
        next.destination = None;
        next.consumed = None;
        next.message = message.clone();
        next.native_telegram_media
            .as_mut()
            .ok_or(Error::Denied)?
            .edit = None;
        let markup = super::keyboard::keyboard(&next)?;
        previous.superseded = true;
        previous
            .native_telegram_media
            .as_mut()
            .ok_or(Error::Denied)?
            .edit = Some(Edit {
            request_hash: mutation::hash(message),
            successor: next.id.clone(),
            receipt: None,
            lock,
        });
        // The first CAS competes with callbacks and edits. A confirmed conflict
        // has made no write and may release the shared destination lock.
        let version = self.save(&previous, Some(candidate.version)).await?;
        let next_version = self
            .save(&next, None)
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(Claim {
            previous,
            version,
            prepared: PreparedCard {
                card: next,
                version: next_version,
                markup,
            },
        })
    }
    pub(crate) async fn finish_telegram_media_edit(
        &self,
        mut claim: Claim,
        receipt: &IntentReceipt,
    ) -> Result<(), Error> {
        claim
            .previous
            .native_telegram_media
            .as_mut()
            .and_then(|a| a.edit.as_mut())
            .ok_or(Error::Denied)?
            .receipt = Some(receipt.clone());
        match self.save(&claim.previous, Some(claim.version)).await {
            Ok(_) => Ok(()),
            Err(error) => {
                let (stored, version) = self.load(&claim.previous.id).await?;
                if Some(version) == claim.version.checked_add(1)
                    && serde_json::to_value(&stored).ok()
                        == serde_json::to_value(&claim.previous).ok()
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}
