//! Explicit WhatsApp templates use the existing authenticated relay reply route.
use super::{DeliveryOutcome, PreparedReply, RelayClient, ReplyAddress, REQUEST_MAX_BYTES};
use cmaeg_core::{channel::Platform, message::WhatsappTemplate};

impl RelayClient {
    pub(crate) async fn send_whatsapp_interactive(
        &self,
        address: &ReplyAddress,
        secret: &cmaeg_core::channel::Secret,
        interactive: &serde_json::Value,
    ) -> (DeliveryOutcome, Option<String>) {
        let ReplyAddress::NyxidRelay {
            platform: Platform::Whatsapp,
            message_id,
            thread_reply: false,
            ..
        } = address
        else {
            return (DeliveryOutcome::Unsupported, None);
        };
        let body = match serde_json::to_vec(&serde_json::json!({"message_id":message_id,
            "reply":{"text":null,"metadata":{"interactive":interactive}}}))
        {
            Ok(body) if body.len() <= 20 * 1024 => body,
            _ => {
                return (
                    DeliveryOutcome::Rejected {
                        reason: "interactive_message_too_large".into(),
                    },
                    None,
                )
            }
        };
        let prepared = PreparedReply {
            body,
            message_id: message_id.clone(),
            thread_reply: false,
            native_media: false,
            complete_text: false,
        };
        let result = self
            .send_prepared(super::Credential::ChannelKey(secret), &prepared)
            .await;
        if matches!(
            &result,
            (
                DeliveryOutcome::Confirmed {
                    platform_message_id: None
                },
                _
            ) | (DeliveryOutcome::Confirmed { .. }, None)
        ) {
            (super::unknown("receipt_invalid"), None)
        } else {
            result
        }
    }

    pub(in crate::delivery) fn prepare_whatsapp_template(
        address: &ReplyAddress,
        template: &WhatsappTemplate,
    ) -> Result<PreparedReply, DeliveryOutcome> {
        let rejected = |reason: &str| DeliveryOutcome::Rejected {
            reason: reason.into(),
        };
        let ReplyAddress::NyxidRelay {
            platform: Platform::Whatsapp,
            message_id,
            thread_reply: false,
            ..
        } = address
        else {
            return Err(rejected("whatsapp_template_destination_unsupported"));
        };
        template
            .validate()
            .map_err(|_| rejected("whatsapp_template_invalid"))?;
        validate_template_links(template)
            .map_err(|_| rejected("whatsapp_template_media_link_rejected"))?;
        // No caller-controlled recipient, route, credential or arbitrary metadata.
        let body = serde_json::to_vec(&serde_json::json!({
            "message_id": message_id,
            "reply": {"text": null, "metadata": {"template": template}}
        }))
        .map_err(|_| rejected("relay_request_invalid"))?;
        if body.len() > REQUEST_MAX_BYTES {
            return Err(rejected("relay_request_too_large"));
        }
        Ok(PreparedReply {
            body,
            message_id: message_id.clone(),
            thread_reply: false,
            native_media: template.has_media(),
            complete_text: false,
        })
    }
}

/// Templates carry references directly to Meta; apply the same conservative
/// public-link policy before either a reply or an owner-initiated send commits.
pub(super) fn validate_template_links(template: &WhatsappTemplate) -> Result<(), ()> {
    for link in template.media_links() {
        let url = crate::net::policy::target(link).ok_or(())?;
        if url.host_str().is_some_and(|host| {
            let host = host.trim_end_matches('.');
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
            return Err(());
        }
    }
    Ok(())
}
