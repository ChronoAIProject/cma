//! One durable replacement claim per native card revision, before any PATCH.
use super::{discord, Card, Error, InteractionStore, PreparedCard};
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, IntentReceipt},
};
use cmaeg_core::{
    activity::ActivityConversation, message::OutboundMessage, seal::MessageRefPayload,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    request_hash: String,
    successor: String,
    receipt: Option<IntentReceipt>,
}
pub(crate) struct Candidate {
    pub card: Card,
    pub conversation: ActivityConversation,
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
fn request_hash(message: &OutboundMessage) -> String {
    format!(
        "{:x}",
        Sha256::digest(cmaeg_core::profile::canonical_json(
            serde_json::to_value(message).expect("outbound message serializes")
        ))
    )
}
pub(super) fn valid(card: &Card) -> bool {
    card.discord_edit.as_ref().is_none_or(|edit| {
        card.discord.is_some()
            && card.superseded
            && edit.request_hash.len() == 64
            && edit.request_hash.bytes().all(|b| b.is_ascii_hexdigit())
            && uuid::Uuid::parse_str(&edit.successor).is_ok()
            && edit.successor != card.id
    })
}
impl InteractionStore {
    /// The sealed reference and saved record, never requested IDs, select the target.
    pub(crate) async fn discord_edit_candidate(
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
        let identity = card.discord.as_ref().ok_or(Error::Denied)?;
        let destination = card.destination.as_ref().ok_or(Error::Denied)?;
        if reference.validate().is_err()
            || reference.discord_message.is_some()
            || card.channel_id != channel.id
            || card.owner != channel.definition.owner.subject
            || card.route_id != reference.route_id
            || card.event_id != reference.event_id
            || card.consumer != reference.consumer
            || card.reference.is_none()
            || destination.platform_id != reference.platform_message_id
            || destination.outbound_id != destination.platform_id
            || discord::source_revision(&channel.definition)? != identity.source_revision
            || channel
                .consumer(Some(&identity.consumer))
                .is_none_or(|consumer| {
                    consumer.pending
                        || consumer.credential.is_none()
                        || consumer.provider != &identity.provider
                })
            || crate::discord::rest::token(channel, &identity.bot_id).is_none()
        {
            return Err(Error::Denied);
        }
        if let Some(edit) = &card.discord_edit {
            if edit.request_hash != request_hash(message) {
                return Err(Error::Denied);
            }
            let receipt = edit.receipt.clone().unwrap_or_else(uncertain);
            if let Some(reference) = &receipt.message_ref {
                let next = reference
                    .open(&self.keys)
                    .map_err(|_| Error::Denied)?
                    .payload;
                if next.interaction_record_id.as_ref() != Some(&edit.successor)
                    || next.channel_id != card.channel_id
                    || next.consumer != card.consumer
                    || next.route_id != card.route_id
                    || next.event_id != card.event_id
                    || next.platform_message_id != destination.platform_id
                    || !matches!(&receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id: Some(id) } if id == &destination.platform_id)
                {
                    return Err(Error::Denied);
                }
            } else if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
                return Err(Error::Denied);
            }
            return Ok(Admission::Replay(receipt));
        }
        if card.superseded {
            return Err(Error::Denied);
        }
        let conversation = ActivityConversation {
            id: card.chat.clone(),
            kind: identity.conversation_kind,
            thread_id: card.thread.clone(),
        };
        Ok(Admission::Ready(Box::new(Candidate {
            card,
            version,
            conversation,
        })))
    }
    pub(crate) async fn begin_discord_edit(
        &self,
        candidate: Candidate,
        message: &OutboundMessage,
    ) -> Result<Claim, Error> {
        let Candidate {
            mut card, version, ..
        } = candidate;
        let mut next = card.clone();
        next.id = uuid::Uuid::new_v4().to_string();
        next.revision = next.revision.checked_add(1).ok_or(Error::Denied)?;
        next.message = message.clone();
        next.consumed = None;
        next.reference = None;
        next.discord_edit = None;
        let markup = discord::render_replacement(message, &next.id, next.revision)?;
        card.superseded = true;
        card.discord_edit = Some(Edit {
            request_hash: request_hash(message),
            successor: next.id.clone(),
            receipt: None,
        });
        // Competing edits and callbacks share this CAS. A loser never writes Discord.
        let version = self.save(&card, Some(version)).await?;
        let next_version = self.save(&next, None).await?;
        Ok(Claim {
            previous: card,
            version,
            prepared: PreparedCard {
                card: next,
                version: next_version,
                markup,
            },
        })
    }
    pub(crate) async fn finish_discord_edit(
        &self,
        mut claim: Claim,
        receipt: &IntentReceipt,
    ) -> Result<(), Error> {
        claim
            .previous
            .discord_edit
            .as_mut()
            .ok_or(Error::Denied)?
            .receipt = Some(receipt.clone());
        self.save(&claim.previous, Some(claim.version)).await?;
        Ok(())
    }
}
pub(crate) fn uncertain() -> IntentReceipt {
    IntentReceipt {
        message_ref: None,
        rendering: cmaeg_core::message::RenderingReport {
            outcome: cmaeg_core::message::RenderingOutcome::Unsupported,
            blocks: vec![],
        },
        delivery: DeliveryOutcome::Unknown {
            reason: "discord_card_edit_in_progress_or_uncertain".into(),
        },
    }
}
