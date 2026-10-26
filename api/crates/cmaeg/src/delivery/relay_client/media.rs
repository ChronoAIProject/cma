//! Native attachments use NyxID's existing bounded materialization API.
use super::{DeliveryOutcome, PreparedReply, RelayClient, ReplyAddress, REQUEST_MAX_BYTES};
use cmaeg_core::{
    channel::Platform,
    message::{validate_native_attachments, NativeAttachment, NativeMediaSource},
};

/// Platforms whose NyxID adapters implement each of the four declared media kinds.
pub fn supported(platform: Platform) -> bool {
    matches!(
        platform,
        Platform::Telegram
            | Platform::Lark
            | Platform::Feishu
            | Platform::Discord
            | Platform::Whatsapp
    )
}

impl RelayClient {
    pub(in crate::delivery) fn prepare_native(
        address: &ReplyAddress,
        text: &str,
        attachments: &[NativeAttachment],
    ) -> Result<PreparedReply, DeliveryOutcome> {
        // Preserve the complete legacy request bytes when native media is absent.
        if attachments.is_empty() {
            return Self::prepare(address, text, None);
        }
        let rejected = |reason: &str| DeliveryOutcome::Rejected {
            reason: reason.into(),
        };
        let ReplyAddress::NyxidRelay {
            platform,
            message_id,
            thread_reply,
            ..
        } = address
        else {
            return Err(rejected("native_media_unsupported"));
        };
        if !supported(*platform) {
            return Err(rejected("native_media_unsupported"));
        }
        validate_native_attachments(attachments).map_err(|_| rejected("native_media_invalid"))?;
        for attachment in attachments {
            if let NativeMediaSource::Url { url } = &attachment.source {
                // Apply the gateway's shared public HTTPS policy before token claim.
                // NyxID resolves again, pins all public addresses and refuses redirects.
                let target = crate::net::policy::target(url)
                    .ok_or_else(|| rejected("native_media_url_rejected"))?;
                if target.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host.ends_with(".localhost")
                        || host.ends_with(".local")
                        || host.ends_with(".internal")
                        || host.ends_with(".svc")
                        || host.ends_with(".cluster.local")
                }) {
                    return Err(rejected("native_media_url_rejected"));
                }
            }
        }
        let mut value = serde_json::json!({"message_id":message_id,"reply":{
            "text": (!text.is_empty()).then_some(text), "metadata":null, "attachments":attachments
        }});
        if *thread_reply {
            value["thread_reply"] = true.into();
        }
        let body = serde_json::to_vec(&value).map_err(|_| rejected("relay_request_invalid"))?;
        if body.len() > REQUEST_MAX_BYTES {
            return Err(rejected("relay_request_too_large"));
        }
        Ok(PreparedReply {
            body,
            message_id: message_id.clone(),
            thread_reply: *thread_reply,
            native_media: true,
        })
    }
}
