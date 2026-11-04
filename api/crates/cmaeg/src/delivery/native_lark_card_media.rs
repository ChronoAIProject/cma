//! Upload complete replacement sets before fencing or superseding a live card.
use super::{
    intents::DeliveryOutcome,
    native_lark_controls::outcome,
    send::{SendContext, Sender},
};
use crate::{
    interactions::{native_lark::Image, Card, InteractionStore},
    native_lark::Client,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{MessageRefPayload, ReplyAddress},
};

fn rejected() -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: "native_lark_card_replacement_invalid_or_revoked".into(),
    }
}

impl Sender {
    pub(super) async fn upload_native_lark_card_images(
        &self,
        ctx: &SendContext<'_>,
        cards: &InteractionStore,
        old: &Card,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> Result<Vec<Image>, DeliveryOutcome> {
        let destination = old
            .native_lark
            .as_ref()
            .and_then(|authority| authority.destination.as_ref())
            .ok_or_else(rejected)?;
        let files = crate::media::outbound::prepare(&message.native_attachments)
            .await
            .map_err(|_| rejected())?;
        let mut uploads = Vec::with_capacity(files.len());
        // Validate the entire set before any upload. A bad later file leaves the
        // original card and every native write untouched.
        for (attachment, file) in message.native_attachments.iter().zip(files) {
            uploads.push(
                Client::prepare_media(attachment, file)
                    .await
                    .map_err(outcome)?,
            );
        }
        let _pace = self
            .pacing
            .acquire(&reference.route_id)
            .await
            .ok_or_else(rejected)?;
        let reply = ReplyAddress::NativeLark(destination.reply.clone());
        let mut images = Vec::with_capacity(uploads.len());
        for upload in uploads {
            let channel = cards
                .refresh_native_lark(old)
                .await
                .map_err(|_| rejected())?;
            let (current, _) = cards
                .native_lark_reference(&channel, reference)
                .await
                .map_err(|_| rejected())?;
            if current.superseded || !crate::interactions::native_lark::mutation_ready(&current) {
                return Err(rejected());
            }
            let credentials = crate::native_lark::credentials(&channel.definition, &reply)
                .ok_or_else(rejected)?;
            let token = self
                .relay
                .native_lark
                .verified_token(&destination.reply.identity, credentials)
                .await
                .map_err(outcome)?;
            self.relay
                .native_lark
                .card_message(&token, &destination.reply)
                .await
                .map_err(outcome)?;
            // Source/app/consumer revocation during token or metadata reads must
            // prevent the next external upload as well as the final PATCH.
            cards
                .refresh_native_lark(&current)
                .await
                .map_err(|_| rejected())?;
            let permit = ctx.gate.attempt().await.map_err(|error| error.outcome())?;
            let uploaded = self
                .relay
                .native_lark
                .upload_media(&token, &destination.reply, upload)
                .await;
            drop(permit);
            ctx.gate.progress();
            let uploaded = uploaded.map_err(outcome)?;
            images.push(
                Image::confirmed(
                    uploaded
                        .card_image_key(&destination.reply)
                        .map_err(outcome)?,
                )
                .map_err(|_| rejected())?,
            );
        }
        // No recipient mutation has occurred; failure, revocation or a callback
        // winning during uploads leaves the original card's authority intact.
        let channel = cards
            .refresh_native_lark(old)
            .await
            .map_err(|_| rejected())?;
        let (current, _) = cards
            .native_lark_reference(&channel, reference)
            .await
            .map_err(|_| rejected())?;
        if current.superseded || !crate::interactions::native_lark::mutation_ready(&current) {
            return Err(rejected());
        }
        Ok(images)
    }
}
