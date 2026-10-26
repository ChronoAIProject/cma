//! Initiated sends name only an already persisted NyxID conversation.
use super::{
    headers, unknown, valid_id, RelayClient, ReplyResponse, RECEIPT_MAX_BYTES, REQUEST_MAX_BYTES,
};
use crate::{
    delivery::{intents::DeliveryOutcome, render::RelayText},
    tools::Capabilities,
};
use cmaeg_core::{
    channel::{Platform, Secret},
    message::{NativeMediaSource, OutboundMessage, RenderingReport},
    problem::{Problem, ProblemCode},
};
use reqwest::StatusCode;

pub(crate) struct PreparedInitiated {
    body: Vec<u8>,
    native_media: bool,
}

impl RelayClient {
    pub(crate) fn prepare_initiated(
        &self,
        platform: Platform,
        route_id: &str,
        key: &str,
        message: &OutboundMessage,
    ) -> Result<(PreparedInitiated, RenderingReport), Problem> {
        let invalid = |detail| Problem::new(ProblemCode::InvalidRequest, 400, detail);
        message
            .validate()
            .map_err(|_| invalid("outbound_message_invalid"))?;
        if message.thread_reply || message.reply_to.is_some() {
            return Err(invalid(
                "initiated_send_has_no_inbound_reply_or_thread_authority",
            ));
        }
        let mut capabilities = Capabilities::relay(platform);
        // An unsolicited send has no admitted event/consumer callback lineage.
        // Report the existing text rendering rather than minting action authority.
        capabilities.buttons = "numbered_text".into();
        let (part, rendering) = RelayText.whole(message, &capabilities);
        if !message.native_attachments.is_empty() {
            if !self.native_media || !super::media::supported(platform) {
                return Err(invalid("native_media_unsupported"));
            }
            for attachment in &message.native_attachments {
                if let NativeMediaSource::Url { url } = &attachment.source {
                    let target = crate::net::policy::target(url)
                        .ok_or_else(|| invalid("native_media_url_rejected"))?;
                    if target.host_str().is_some_and(|host| {
                        host == "localhost"
                            || [
                                ".localhost",
                                ".local",
                                ".internal",
                                ".svc",
                                ".cluster.local",
                            ]
                            .iter()
                            .any(|suffix| host.ends_with(suffix))
                    }) {
                        return Err(invalid("native_media_url_rejected"));
                    }
                }
            }
        }
        let metadata = if let Some(template) = &message.whatsapp_template {
            if platform != Platform::Whatsapp || !self.whatsapp_templates {
                return Err(invalid("whatsapp_template_unsupported"));
            }
            super::whatsapp::validate_template_links(template)
                .map_err(|_| invalid("whatsapp_template_media_link_rejected"))?;
            Some(serde_json::json!({"template":template}))
        } else {
            None
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "conversation_id":route_id, "idempotency_key":key,
            "message":{"text":(!part.text.is_empty()).then_some(part.text), "metadata":metadata, "attachments":message.native_attachments}
        })).map_err(|_| invalid("outbound_message_invalid"))?;
        if body.len() > REQUEST_MAX_BYTES {
            return Err(invalid("relay_request_too_large"));
        }
        Ok((
            PreparedInitiated {
                body,
                native_media: !message.native_attachments.is_empty(),
            },
            rendering,
        ))
    }
    /// One exchange only. Even a 429 can follow partial upstream acceptance.
    pub(crate) async fn send_initiated(
        &self,
        secret: &Secret,
        prepared: PreparedInitiated,
    ) -> DeliveryOutcome {
        let Some(headers) = headers(secret) else {
            return DeliveryOutcome::Rejected {
                reason: "no_credential".into(),
            };
        };
        let mut endpoint = self.endpoint.clone();
        endpoint.set_path("/api/v1/channel-relay/send");
        let timeout = if prepared.native_media {
            std::time::Duration::from_secs(60)
        } else {
            self.timeout
        };
        let result = tokio::time::timeout(
            timeout,
            self.transport
                .post(endpoint, headers, prepared.body, timeout),
        )
        .await;
        let Ok(Ok(reply)) = result else {
            return unknown("response_lost");
        };
        if reply.status != StatusCode::OK {
            return unknown("provider_failure");
        }
        let Ok(bytes) = reply.body else {
            return unknown("receipt_invalid");
        };
        if bytes.len() > RECEIPT_MAX_BYTES {
            return unknown("receipt_invalid");
        }
        let Ok(receipt) = serde_json::from_slice::<ReplyResponse>(&bytes) else {
            return unknown("receipt_invalid");
        };
        if !valid_id(&receipt.message_id)
            || (prepared.native_media && receipt.platform_message_id.is_none())
            || receipt
                .platform_message_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
        {
            return unknown("receipt_invalid");
        }
        DeliveryOutcome::Confirmed {
            platform_message_id: receipt.platform_message_id,
        }
    }
}
