//! A confirmed app-scoped image is retained with the existing durable card authority.
use super::super::{Error, InteractionStore, PreparedCard};
use cmaeg_core::{
    channel::lark_resources::safe_id,
    message::{NativeMediaKind, OutboundMessage, RenderingReport},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Image {
    key: String,
}
impl Image {
    pub(crate) fn confirmed(key: String) -> Result<Self, Error> {
        if !safe_id(&key, 1024) {
            return Err(Error::Denied);
        }
        Ok(Self { key })
    }
}

/// Validate the combined request before fetching its source or making any write.
/// Raw attachment sources are deliberately excluded from the retained card.
pub(crate) fn preflight_media(message: &OutboundMessage) -> Result<OutboundMessage, Error> {
    if message.validate().is_err()
        || message.native_attachments.len() != 1
        || message.native_attachments[0].kind != NativeMediaKind::Image
        || message.native_attachments[0].caption.is_some()
        || super::super::lark_form::requested(message)
        || !super::super::has_controls(message)
    {
        return Err(Error::Denied);
    }
    let mut retained = message.clone();
    retained.native_attachments.clear();
    // Reserve the maximum confirmed key size and one image element up front.
    render_with_image(
        &retained,
        "00000000000040008000000000000000",
        1,
        false,
        Some(&Image {
            key: "i".repeat(1024),
        }),
    )?;
    Ok(retained)
}

pub(crate) fn render_with_image(
    message: &OutboundMessage,
    id: &str,
    revision: u64,
    form: bool,
    image: Option<&Image>,
) -> Result<(Value, RenderingReport), Error> {
    let (mut card, report) = super::render(message, id, revision, form)?;
    if let Some(image) = image {
        if form || !safe_id(&image.key, 1024) {
            return Err(Error::Denied);
        }
        let elements = card["body"]["elements"]
            .as_array_mut()
            .ok_or(Error::Denied)?;
        if elements.len() >= 100 {
            return Err(Error::Denied);
        }
        elements.insert(
            0,
            json!({
                "tag":"img", "img_key":image.key,
                "alt":{"tag":"plain_text","content":""},
                "scale_type":"fit_horizontal", "preview":true
            }),
        );
        if serde_json::to_vec(&card).map_err(|_| Error::Denied)?.len() > 28 * 1024 {
            return Err(Error::Denied);
        }
    }
    Ok((card, report))
}

impl InteractionStore {
    /// Called only with a confirmed upload bound to the original native destination.
    /// Persist the key and its current custody before sending the interactive card.
    pub(crate) async fn attach_native_lark_image(
        &self,
        prepared: &mut PreparedCard,
        uploaded: crate::native_lark::outbound::UploadedMedia,
    ) -> Result<(), Error> {
        let channel = self.refresh_native_lark(&prepared.card).await?;
        let a = prepared.card.native_lark.as_mut().ok_or(Error::Denied)?;
        if a.image.is_some()
            || a.destination.is_some()
            || a.replacement.is_some()
            || prepared.card.reference.is_some()
            || prepared.card.superseded
            || prepared.card.consumed.is_some()
        {
            return Err(Error::Denied);
        }
        a.image = Some(Image::confirmed(
            uploaded
                .card_image_key(a.reply())
                .map_err(|_| Error::Denied)?,
        )?);
        let custody = prepared
            .card
            .native_lark
            .as_ref()
            .unwrap()
            .digest(&prepared.card, &channel)
            .ok_or(Error::Denied)?;
        prepared.card.native_lark.as_mut().unwrap().custody = custody;
        prepared.markup = super::markup(&prepared.card)?.0;
        prepared.version = self.save(&prepared.card, Some(prepared.version)).await?;
        Ok(())
    }
}
