//! Platform message editing through exact NyxID outbound receipts.
use super::*;

impl RelayClient {
    /// Ordinary text updates require a correlated receipt. A successful HTTP
    /// status by itself is insufficient, and ambiguous attempts are not replayed.
    pub async fn edit_text(
        &self,
        secret: &Secret,
        platform_id: &str,
        outbound_id: &str,
        text: &str,
    ) -> DeliveryOutcome {
        self.edit_text_with_commonmark(secret, platform_id, outbound_id, text, false)
            .await
    }

    pub async fn edit_text_with_commonmark(
        &self,
        secret: &Secret,
        platform_id: &str,
        outbound_id: &str,
        text: &str,
        commonmark: bool,
    ) -> DeliveryOutcome {
        if !valid_id(platform_id) || !valid_id(outbound_id) || text.is_empty() {
            return DeliveryOutcome::Rejected {
                reason: "relay_edit_invalid".into(),
            };
        }
        let Ok(body) = serde_json::to_vec(&serde_json::json!({
            "message_id": platform_id,
            "outbound_message_id": outbound_id,
            "reply": {"text": text}
        })) else {
            return unknown("edit_unconfirmed");
        };
        self.post_text_edit(secret, platform_id, body, commonmark)
            .await
    }

    pub(crate) async fn edit_with_commonmark(
        &self,
        secret: &Secret,
        destination: &crate::interactions::Destination,
        text: &str,
        markup: &serde_json::Value,
        commonmark: bool,
    ) -> DeliveryOutcome {
        if !commonmark {
            return self.edit(secret, destination, text, markup).await;
        }
        if !valid_id(&destination.platform_id)
            || !valid_id(&destination.outbound_id)
            || text.is_empty()
        {
            return DeliveryOutcome::Rejected {
                reason: "relay_edit_invalid".into(),
            };
        }
        let Ok(body) = serde_json::to_vec(&serde_json::json!({
            "message_id": destination.platform_id,
            "outbound_message_id": destination.outbound_id,
            "reply": {"text": text, "metadata": {"reply_markup": markup}}
        })) else {
            return unknown("edit_unconfirmed");
        };
        self.post_text_edit(secret, &destination.platform_id, body, true)
            .await
    }

    async fn post_text_edit(
        &self,
        secret: &Secret,
        platform_id: &str,
        body: Vec<u8>,
        commonmark: bool,
    ) -> DeliveryOutcome {
        let mut endpoint = self.endpoint.clone();
        endpoint.set_path(if commonmark {
            "/api/v1/channel-relay/reply/update/commonmark"
        } else {
            "/api/v1/channel-relay/reply/update"
        });
        let Some(headers) = headers(secret) else {
            return unknown("edit_unconfirmed");
        };
        if body.len() > REQUEST_MAX_BYTES {
            return DeliveryOutcome::Rejected {
                reason: "relay_request_too_large".into(),
            };
        }
        let exchange = self.transport.post(endpoint, headers, body, self.timeout);
        let Ok(Ok(reply)) = tokio::time::timeout(self.timeout, exchange).await else {
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
        let Ok(bytes) = reply.body else {
            return unknown("edit_unconfirmed");
        };
        if bytes.len() > RECEIPT_MAX_BYTES {
            return unknown("receipt_invalid");
        }
        #[derive(Deserialize)]
        struct Receipt {
            upstream_message_id: String,
            edited_at: String,
        }
        let Ok(receipt) = serde_json::from_slice::<Receipt>(&bytes) else {
            return unknown("receipt_invalid");
        };
        if receipt.upstream_message_id != platform_id
            || time::OffsetDateTime::parse(
                &receipt.edited_at,
                &time::format_description::well_known::Rfc3339,
            )
            .is_err()
        {
            return unknown("receipt_invalid");
        }
        DeliveryOutcome::Confirmed {
            platform_message_id: Some(platform_id.into()),
        }
    }

    pub async fn edit(
        &self,
        secret: &Secret,
        destination: &crate::interactions::Destination,
        text: &str,
        markup: &serde_json::Value,
    ) -> DeliveryOutcome {
        let Ok(endpoint) = self.endpoint.join("reply/update") else {
            return DeliveryOutcome::Unsupported;
        };
        let Some(headers) = headers(secret) else {
            return unknown("edit_unconfirmed");
        };
        let Ok(body) = serde_json::to_vec(
            &serde_json::json!({"message_id":destination.platform_id,"outbound_message_id":destination.outbound_id,"reply":{"text":text,"metadata":{"reply_markup":markup}}}),
        ) else {
            return unknown("edit_unconfirmed");
        };
        match self
            .transport
            .post(endpoint, headers, body, self.timeout)
            .await
        {
            Ok(reply) if reply.status.is_success() => DeliveryOutcome::Confirmed {
                platform_message_id: Some(destination.platform_id.clone()),
            },
            Ok(reply) if reply.status == StatusCode::TOO_MANY_REQUESTS => {
                DeliveryOutcome::RateLimited {
                    retry_after: retry_after(&reply.headers),
                }
            }
            _ => unknown("edit_unconfirmed"),
        }
    }
}
