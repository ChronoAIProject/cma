//! Lark metadata.card transport with exact send/edit receipts.
use super::*;
use crate::interactions::Destination;
use serde_json::{json, Value};

impl RelayClient {
    /// Gate new card issuance until every reader understands retained Lark cards.
    pub fn with_lark_cards(mut self, enabled: bool) -> Self {
        self.lark_cards = enabled;
        self
    }
    pub(in crate::delivery) fn lark_cards_enabled(&self) -> bool {
        self.lark_cards
    }
    /// Stop new form issuance independently of retained callback recovery.
    pub fn with_lark_forms(mut self, enabled: bool) -> Self {
        self.lark_forms = enabled;
        self
    }
    pub(in crate::delivery) fn lark_forms_enabled(&self) -> bool {
        self.lark_forms
    }
    pub(crate) async fn send_lark_card(
        &self,
        address: &ReplyAddress,
        secret: &Secret,
        card: &Value,
    ) -> (DeliveryOutcome, Option<String>) {
        let ReplyAddress::NyxidRelay {
            message_id,
            platform,
            thread_reply: false,
            ..
        } = address
        else {
            return (DeliveryOutcome::Unsupported, None);
        };
        if !crate::interactions::lark::supported(*platform) {
            return (DeliveryOutcome::Unsupported, None);
        }
        let body = match serde_json::to_vec(&json!({"message_id":message_id,
            "reply":{"text":null,"metadata":{"card":card}}}))
        {
            Ok(body) if body.len() <= 32 * 1024 => body,
            _ => {
                return (
                    DeliveryOutcome::Rejected {
                        reason: "interactive_card_too_large".into(),
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
            .send_prepared(Credential::ChannelKey(secret), &prepared)
            .await;
        if matches!(
            result.0,
            DeliveryOutcome::Confirmed {
                platform_message_id: None
            }
        ) {
            (unknown("receipt_invalid"), None)
        } else {
            result
        }
    }

    pub(crate) async fn edit_lark_card(
        &self,
        secret: &Secret,
        destination: &Destination,
        card: &Value,
    ) -> DeliveryOutcome {
        if !valid_id(&destination.platform_id) || !valid_id(&destination.outbound_id) {
            return DeliveryOutcome::Rejected {
                reason: "relay_edit_invalid".into(),
            };
        }
        let Ok(endpoint) = self.endpoint.join("reply/update") else {
            return DeliveryOutcome::Unsupported;
        };
        let Some(headers) = headers(secret) else {
            return unknown("edit_unconfirmed");
        };
        let body = match serde_json::to_vec(&json!({"message_id":destination.platform_id,
            "outbound_message_id":destination.outbound_id,"reply":{"text":null,"metadata":{"card":card}}}))
        {
            Ok(body) if body.len() <= 32 * 1024 => body,
            _ => {
                return DeliveryOutcome::Rejected {
                    reason: "interactive_card_too_large".into(),
                }
            }
        };
        let Ok(Ok(reply)) = tokio::time::timeout(
            self.timeout,
            self.transport.post(endpoint, headers, body, self.timeout),
        )
        .await
        else {
            return unknown("edit_unconfirmed");
        };
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            return DeliveryOutcome::RateLimited {
                retry_after: retry_after(&reply.headers),
            };
        }
        if reply.status != StatusCode::OK {
            return unknown("edit_unconfirmed");
        }
        let Ok(body) = reply.body else {
            return unknown("edit_unconfirmed");
        };
        if body.len() > RECEIPT_MAX_BYTES {
            return unknown("receipt_invalid");
        }
        let Ok(receipt) = serde_json::from_slice::<Value>(&body) else {
            return unknown("receipt_invalid");
        };
        if receipt["upstream_message_id"] != destination.platform_id
            || receipt["edited_at"].as_str().is_none_or(|value| {
                time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .is_err()
            })
        {
            return unknown("receipt_invalid");
        }
        DeliveryOutcome::Confirmed {
            platform_message_id: Some(destination.platform_id.clone()),
        }
    }
}
