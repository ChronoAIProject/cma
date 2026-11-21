//! Matching native/relay locator validation.
use super::*;

pub(super) fn validate_locator(
    locator: &AttachmentSource,
    source: &SourceKind,
) -> Result<(), String> {
    match (locator, source) {
        (
            AttachmentSource::NativeLark {
                message_id,
                resource_key,
                source_revision,
                ..
            },
            SourceKind::NativeLark(identity),
        ) => {
            use crate::channel::lark_resources::safe_id;
            if identity.validate().is_err()
                || !message_id.starts_with("om_")
                || !safe_id(message_id, 128)
                || !safe_id(resource_key, 1024)
                || source_revision.len() != 64
                || !source_revision
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("invalid native Lark resource identity".into());
            }
        }
        (
            AttachmentSource::Whatsapp {
                message_id,
                media_id,
                sha256,
            },
            SourceKind::Whatsapp { .. },
        ) => {
            if !crate::channel::whatsapp::valid_message_id(message_id)
                || !crate::channel::whatsapp::valid_meta_id(media_id)
                || sha256.len() != 64
                || !sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("invalid native WhatsApp media identity".into());
            }
        }
        (
            AttachmentSource::LarkResource {
                message_id,
                resource_key,
                resource_kind,
                binding,
                bot_id,
                source_revision,
                ..
            },
            SourceKind::NyxidRelay { platform, .. },
        ) => {
            use crate::channel::lark_resources::{lark_resource_service, safe_id};
            if lark_resource_service(*platform).is_none()
                || !message_id.starts_with("om_")
                || !safe_id(message_id, 128)
                || !safe_id(resource_key, 1024)
                || !match resource_kind {
                    crate::channel::LarkResourceKind::Image => resource_key.starts_with("img_"),
                    crate::channel::LarkResourceKind::File => resource_key.starts_with("file_"),
                }
                || !bot_id.starts_with("ou_")
                || !safe_id(bot_id, 128)
                || binding.validate("/binding").is_err()
                || source_revision.len() != 64
                || !source_revision.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("invalid Lark resource identity".into());
            }
        }
        (AttachmentSource::NyxidRelay { index }, SourceKind::NyxidRelay { .. }) => {
            if *index > MAX_NYXID_INDEX {
                return Err(format!("relay index must be at most {MAX_NYXID_INDEX}"));
            }
        }
        (
            AttachmentSource::Telegram {
                file_id,
                file_unique_id,
            },
            SourceKind::Telegram { .. },
        ) => {
            if !telegram_ids_valid(file_id, file_unique_id) {
                return Err("Telegram file identifiers must match [A-Za-z0-9_-]".into());
            }
        }
        (
            AttachmentSource::Discord {
                channel_id,
                message_id,
                attachment_id,
            },
            SourceKind::Discord { .. },
        ) => {
            if ![channel_id, message_id, attachment_id]
                .into_iter()
                .all(|id| crate::channel::discord::valid_id(id))
            {
                return Err("invalid Discord attachment identity".into());
            }
        }
        (AttachmentSource::Url { url }, SourceKind::Custom) => {
            if !https_locator(url) {
                return Err(format!(
                    "must be an absolute https URL of at most {MAX_ATTACHMENT_URL_BYTES} bytes with a host and no userinfo or fragment"
                ));
            }
        }
        (AttachmentSource::Inline { pointer }, SourceKind::Custom) => {
            if !json_pointer(pointer) {
                return Err(format!(
                    "must be an RFC 6901 JSON pointer of 1 to {MAX_INLINE_POINTER_BYTES} bytes"
                ));
            }
        }
        _ => return Err("locator type does not match the activity source".into()),
    }
    Ok(())
}
