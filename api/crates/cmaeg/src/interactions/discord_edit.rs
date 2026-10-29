//! One durable replacement claim per native card revision, before any PATCH.
use super::{discord, Card, Error, InteractionStore, PreparedCard};
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, IntentReceipt},
};
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    request_hash: String,
    successor: String,
    receipt: Option<IntentReceipt>,
}
pub(crate) use super::discord_authority::Candidate;
pub(crate) enum Admission {
    Ready(Box<Candidate>),
    Replay(IntentReceipt),
}
pub(crate) struct Claim {
    previous: Card,
    version: u64,
    pub prepared: PreparedCard,
}
/// Compact candidate identity is created before serializing the native upload.
pub(crate) struct Replacement {
    id: String,
    request_hash: String,
    media: Vec<super::discord_media::Media>,
}
impl Replacement {
    pub(crate) fn prepare(
        message: &OutboundMessage,
        files: &[crate::media::outbound::PreparedFile],
    ) -> Result<Self, Error> {
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            request_hash: request_hash(message),
            media: super::discord_media::Media::prepare_set(message, files, true)?,
        })
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
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
        let candidate = self.discord_card_candidate(channel, reference).await?;
        let card = &candidate.card;
        validate_edit(card, message)?;
        let destination = card.destination.as_ref().ok_or(Error::Denied)?;
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
        Ok(Admission::Ready(Box::new(candidate)))
    }
    pub(crate) async fn begin_discord_edit(
        &self,
        candidate: Candidate,
        message: &OutboundMessage,
    ) -> Result<Claim, Error> {
        if !message.native_attachments.is_empty() || message.clear_native_attachments {
            return Err(Error::Denied);
        }
        self.begin_discord_revision(candidate, message, None).await
    }
    pub(crate) async fn begin_discord_media_replacement(
        &self,
        candidate: Candidate,
        message: &OutboundMessage,
        replacement: Replacement,
    ) -> Result<Claim, Error> {
        if replacement.request_hash != request_hash(message) {
            return Err(Error::Denied);
        }
        self.begin_discord_revision(candidate, message, Some(replacement))
            .await
    }
    async fn begin_discord_revision(
        &self,
        candidate: Candidate,
        message: &OutboundMessage,
        replacement: Option<Replacement>,
    ) -> Result<Claim, Error> {
        let Candidate {
            mut card, version, ..
        } = candidate;
        validate_edit(&card, message)?;
        if card.superseded
            || card.discord_edit.is_some()
            || replacement.is_some()
                != (!message.native_attachments.is_empty() || message.clear_native_attachments)
        {
            return Err(Error::Denied);
        }
        let mut next = card.clone();
        next.id = replacement
            .as_ref()
            .map(|r| r.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        next.revision = next.revision.checked_add(1).ok_or(Error::Denied)?;
        next.message = message.clone();
        next.message.native_attachments.clear();
        next.message.clear_native_attachments = false;
        next.consumed = None;
        next.reference = None;
        next.discord_edit = None;
        next.discord_form_launch = None;
        if let Some(identity) = &mut next.discord {
            identity.form = super::discord_form::authority(message).ok();
            if let Some(replacement) = replacement {
                identity.set_media(replacement.media)?;
            }
        }
        if let Some(state) = &mut next.discord_effect {
            state.deletion = None;
        }
        let markup = if next
            .discord
            .as_ref()
            .is_some_and(|identity| identity.form.is_some())
        {
            super::discord_form::button(&next)?
        } else {
            discord::render_replacement(&next.message, &next.id, next.revision)?
        };
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
/// Upload replacement requires previously confirmed same-card media authority.
/// Replay checks this immutable request before any source URL is materialized again.
fn validate_edit(card: &Card, message: &OutboundMessage) -> Result<(), Error> {
    message.validate().map_err(|_| Error::Denied)?;
    let identity = card.discord.as_ref().ok_or(Error::Denied)?;
    let media = identity.media_items()?;
    if media.iter().any(|media| !media.has_edit_authority())
        || ((!message.native_attachments.is_empty() || message.clear_native_attachments)
            && media.is_empty())
        || (!media.is_empty()
            && message
                .blocks
                .iter()
                .any(|block| matches!(block, cmaeg_core::message::Block::Form { .. })))
    {
        return Err(Error::Denied);
    }
    Ok(())
}
pub(crate) fn uncertain() -> IntentReceipt {
    IntentReceipt {
        message_parts: vec![],
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
