//! Outer tool waits include queueing/pacing before the provider exchange.
use crate::delivery::{intents::DeliveryCommand, relay_client::COMPLETE_REPLY_TOOL_TIMEOUT};
use cmaeg_core::{channel::Platform, seal::ReplyAddress};
use std::time::Duration;

pub(super) fn for_command(
    command: &DeliveryCommand,
    complete_replies: bool,
    commonmark_relay: bool,
) -> Duration {
    // Template media lives in components, not native_attachments. Both need
    // room for the 60 s exchange; routed replies keep their separate 20 s window.
    if matches!(command, DeliveryCommand::Reply { message, .. }
        if !message.native_attachments.is_empty()
            || message.whatsapp_template.as_ref().is_some_and(|template| template.has_media()))
    {
        Duration::from_secs(65)
    } else if matches!(command, DeliveryCommand::Reply { event, message, .. }
        if matches!(event.reply, ReplyAddress::NyxidRelay { platform, .. }
            if complete_replies || (commonmark_relay && platform == Platform::Telegram))
            && message.whatsapp_template.is_none())
    {
        COMPLETE_REPLY_TOOL_TIMEOUT
    } else {
        Duration::from_secs(8)
    }
}
