//! Edit one confirmed media card without re-uploading or invoking text edits.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
    telegram::{outcome, token},
};
use crate::{
    interactions::{
        telegram_media_edit::{self as cards, Admission},
        Error,
    },
    native_media::model::{Presentation, Snapshot},
};
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};

fn preserved(previous: &Snapshot, observed: &Snapshot, caption: &str) -> bool {
    previous.same_files(observed)
        && observed.caption == caption
        && match (&previous.presentation, &observed.presentation) {
            (
                Some(Presentation::Telegram {
                    entities: before,
                    caption_above: before_above,
                    spoiler: before_spoiler,
                }),
                Some(Presentation::Telegram {
                    entities: after,
                    caption_above: after_above,
                    spoiler: after_spoiler,
                }),
            ) => {
                before_above == after_above
                    && before_spoiler == after_spoiler
                    && (previous.caption != caption || before == after)
            }
            _ => false,
        }
}
impl Sender {
    pub(super) async fn telegram_media_card_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let refuse = || {
            cards::receipt(
                message,
                DeliveryOutcome::Rejected {
                    reason: "interaction_expired_or_invalid".into(),
                },
            )
        };
        let Some(store) = &self.relay.interactions else {
            return refuse();
        };
        let candidate = match store
            .telegram_media_edit_candidate(ctx.channel, reference, message)
            .await
        {
            Ok(Admission::Replay(receipt)) => return receipt,
            Ok(Admission::Ready(candidate)) => candidate,
            Err(Error::Unavailable) => return cards::uncertain(message),
            Err(_) => return refuse(),
        };
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(ticket) => ticket,
            Err(error) => return cards::receipt(message, error.outcome()),
        };
        let Some(mut slot) = self.pacing.acquire(&reference.route_id).await else {
            return refuse();
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(error) => return cards::receipt(message, error.outcome()),
        };
        if !ctx.gate.commit() {
            return cards::receipt(message, expired());
        }
        let serial = store.telegram_media_mutations();
        let held = match serial
            .acquire(
                candidate.destination(),
                &candidate.channel.definition.owner.subject,
                candidate.card.expires_at,
            )
            .await
        {
            Ok(Some(held)) => held,
            Ok(None) => return cards::uncertain(message),
            Err(_) => return cards::uncertain(message),
        };
        // A different replica or callback may have advanced this card while the
        // destination lock was being acquired. Refresh before the shared CAS.
        let candidate = match store
            .telegram_media_edit_candidate(ctx.channel, reference, message)
            .await
        {
            Ok(Admission::Ready(candidate)) => candidate,
            Ok(Admission::Replay(receipt)) => {
                let _ = serial.release(held).await;
                return receipt;
            }
            Err(error) => {
                let _ = serial.release(held).await;
                return if matches!(error, Error::Unavailable) {
                    cards::uncertain(message)
                } else {
                    refuse()
                };
            }
        };
        let bot = candidate.bot().to_owned();
        let chat = candidate.chat().to_owned();
        let thread = candidate.thread().map(str::to_owned);
        let snapshot = candidate.snapshot().clone();
        let caption = candidate.caption.clone();
        let destination = candidate
            .card
            .destination
            .clone()
            .expect("checked destination");
        let mut claim = match store
            .begin_telegram_media_edit(*candidate, message, serial.retain(&held))
            .await
        {
            Ok(claim) => claim,
            Err(Error::Denied) => {
                let _ = serial.release(held).await;
                return cards::uncertain(message);
            }
            Err(_) => return cards::uncertain(message),
        };
        let result = async {
            let current = store
                .telegram_media_channel(&claim.prepared.card)
                .await
                .map_err(|_| crate::telegram::Error::Rejected)?;
            let credential = token(&current, &bot).ok_or(crate::telegram::Error::Rejected)?;
            let edit = self
                .relay
                .telegram
                .prepare_media_card_edit(
                    &chat,
                    &destination.platform_id,
                    &snapshot,
                    &caption,
                    &claim.prepared.markup,
                )
                .await?;
            let value = self.relay.telegram.edit_media(credential, edit).await?;
            let observed = crate::telegram::media_card_edit::observe(
                &value,
                &bot,
                &chat,
                thread.as_deref(),
                &destination.platform_id,
                snapshot.kind,
                &claim.prepared.markup,
            )?;
            if !preserved(&snapshot, &observed, &caption) {
                return Err(crate::telegram::Error::Unconfirmed);
            }
            Ok(observed)
        }
        .await;
        drop(permit);
        ctx.gate.progress();
        let mut receipt = match result {
            Ok(observed) => {
                let a = claim
                    .prepared
                    .card
                    .native_telegram_media
                    .as_mut()
                    .expect("media card");
                a.caption = observed.caption.clone();
                a.snapshot = Some(observed);
                match store
                    .bind_telegram_media(&mut claim.prepared, destination)
                    .await
                {
                    Ok(reference) => {
                        let mut receipt = cards::receipt(
                            message,
                            DeliveryOutcome::Confirmed {
                                platform_message_id: Some(
                                    reference
                                        .open(&self.keys)
                                        .expect("sealed reference")
                                        .payload
                                        .platform_message_id,
                                ),
                            },
                        );
                        receipt.message_ref = Some(reference);
                        receipt
                    }
                    Err(_) => cards::uncertain(message),
                }
            }
            Err(error) => cards::receipt(message, outcome(error)),
        };
        if let DeliveryOutcome::RateLimited { retry_after } = receipt.delivery {
            slot.cooldown(retry_after);
        }
        if store
            .finish_telegram_media_edit(claim, &receipt)
            .await
            .is_err()
        {
            receipt = cards::uncertain(message);
        }
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) {
            let _ = serial.release(held).await;
        }
        receipt
    }
}
