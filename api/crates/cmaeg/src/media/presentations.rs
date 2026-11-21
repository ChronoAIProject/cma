//! Explicit presentation requests never alter generic audio/video semantics.
use cmaeg_core::{
    message::{NativeMediaKind, OutboundMessage},
    seal::ReplyAddress,
};

/// Check before any transport effect. Durable idempotency replay remains available
/// when issuance is disabled: retained operation records contain hashes, not new kinds.
/// Legacy WhatsApp stickers keep their independent existing media gate.
pub(crate) fn allowed(
    enabled: bool,
    destination: &ReplyAddress,
    message: &OutboundMessage,
) -> bool {
    message.native_attachments.iter().all(|item| {
        let new_presentation = match item.kind {
            NativeMediaKind::Voice => {
                if !matches!(
                    destination,
                    ReplyAddress::Telegram { .. } | ReplyAddress::Whatsapp { .. }
                ) {
                    return false;
                }
                true
            }
            NativeMediaKind::Animation => {
                if !matches!(destination, ReplyAddress::Telegram { .. }) {
                    return false;
                }
                true
            }
            NativeMediaKind::Sticker => match destination {
                ReplyAddress::Telegram { .. } => true,
                ReplyAddress::Whatsapp { .. } => return true,
                _ => return false,
            },
            _ => false,
        };
        !new_presentation || (enabled && !crate::interactions::has_controls(message))
    })
}

pub(crate) fn special(kind: NativeMediaKind) -> bool {
    matches!(
        kind,
        NativeMediaKind::Voice | NativeMediaKind::Animation | NativeMediaKind::Sticker
    )
}

#[cfg(test)]
#[path = "presentations_tests.rs"]
mod tests;
