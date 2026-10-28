//! References for individually confirmed ordinary native text parts.
use super::send::Sender;
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    ids::MessageRef,
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) fn telegram_text_ref(
        &self,
        event: &EventRefPayload,
        platform_message_id: &str,
    ) -> Option<MessageRef> {
        let ReplyAddress::Telegram {
            bot_id, chat_id, ..
        } = &event.reply
        else {
            return None;
        };
        MessageRef::seal(
            &MessageRefPayload {
                discord_message: None,
                native_media_edit: None,
                received_reaction: None,
                native_lark_effect: None,
                whatsapp_reaction: None,
                lark_effect: None,
                telegram_effect: None,
                channel_id: event.channel_id.clone(),
                event_id: event.event_id.clone(),
                platform_message_id: platform_message_id.into(),
                route_id: crate::telegram::route(bot_id, chat_id),
                interaction_record_id: None,
                relay_message: None,
                consumer: event.consumer.clone(),
            },
            &self.keys,
        )
        .ok()
    }
    pub(super) fn telegram_part_ref(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        platform_message_id: &str,
        issued_at: i64,
    ) -> Option<MessageRef> {
        if event.channel_id != channel.id || event.record_id != channel.pointer.record_id {
            return None;
        }
        let reference = self.telegram_text_ref(event, platform_message_id)?;
        if self.relay.telegram_effects_enabled() {
            if let Some(enriched) = crate::tools::message_effects::authority::enrich(
                &reference, event, channel, &self.keys, issued_at, false,
            ) {
                return Some(enriched);
            }
        }
        Some(reference)
    }
}
