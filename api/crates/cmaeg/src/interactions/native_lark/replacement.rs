use super::super::{Error, InteractionStore, PreparedCard};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Patch {
    pub original_card: String,
    pub confirmed: bool,
}
impl InteractionStore {
    pub(crate) async fn native_lark_patch_confirmed(
        &self,
        prepared: &mut PreparedCard,
    ) -> Result<(), Error> {
        let patch = prepared
            .card
            .native_lark
            .as_mut()
            .and_then(|a| a.replacement.as_mut())
            .ok_or(Error::Denied)?;
        if patch.confirmed {
            return Err(Error::Denied);
        }
        patch.confirmed = true;
        prepared.version = self.save(&prepared.card, Some(prepared.version)).await?;
        Ok(())
    }

    /// Caller holds the shared exact-message mutation fence. Original TTL is immutable.
    pub(crate) async fn replace_native_lark(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        let (mut old, version) = self.native_lark_reference(channel, reference).await?;
        if old.superseded
            || message.validate().is_err()
            || (message.thread_reply && old.thread.is_none())
        {
            return Err(Error::Denied);
        }
        let form = old.native_lark.as_ref().ok_or(Error::Denied)?.form;
        if super::super::lark_form::requested(message) != form {
            return Err(Error::Denied);
        }
        let revision = old.revision.checked_add(1).ok_or(Error::Denied)?;
        super::render(message, &old.id, revision, form)?;
        old.superseded = true;
        self.save(&old, Some(version)).await?;
        let original_card = old.id.clone();
        let mut card = old;
        card.id = uuid::Uuid::new_v4().to_string();
        card.revision = revision;
        card.message = message.clone();
        card.superseded = false;
        card.reference = None;
        card.consumed = None;
        let a = card.native_lark.as_mut().unwrap();
        a.pending = None;
        a.destination = None;
        a.replacement = Some(Patch {
            original_card,
            confirmed: false,
        });
        let custody = card
            .native_lark
            .as_ref()
            .unwrap()
            .digest(&card, channel)
            .ok_or(Error::Denied)?;
        card.native_lark.as_mut().unwrap().custody = custody;
        let markup = super::render(message, &card.id, card.revision, form)?.0;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
    }
}
