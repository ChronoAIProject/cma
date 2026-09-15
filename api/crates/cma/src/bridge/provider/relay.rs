use layer2_core::bridge::outbox::{
    DeliveryFailure, DeliveryOutcome, DeliveryReceipt, RelayDeliveryRequest, RelayOperation,
};
use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyResponse {
    message_id: String,
    platform_message_id: Option<String>,
}

impl NyxBridgeProvider {
    pub(crate) async fn reply(
        &self,
        request: &RelayDeliveryRequest,
        secret: &SecretValue,
    ) -> DeliveryOutcome {
        let connector = &request.item().source.config().connector;
        if request.require_current().is_err()
            || !raw_agent_key(secret.expose())
            || connector.base_url.trim_end_matches('/')
                != self.base_url.as_str().trim_end_matches('/')
            || connector.issuer.trim_end_matches('/')
                != self.base_url.as_str().trim_end_matches('/')
        {
            return DeliveryOutcome::DefinitiveRejection(DeliveryFailure::CredentialRevoked);
        }
        if !matches!(request.item().operation, RelayOperation::InitialReply) {
            return DeliveryOutcome::DefinitiveRejection(DeliveryFailure::CapabilityUnsupported);
        }
        self.post_reply(
            request.item().source.message_id(),
            request.item().text(),
            secret,
            request.deadline(),
        )
        .await
    }

    async fn post_reply(
        &self,
        message_id: &str,
        text: &str,
        secret: &SecretValue,
        deadline: tokio::time::Instant,
    ) -> DeliveryOutcome {
        let result = tokio::time::timeout_at(deadline, async {
            let mut endpoint = self.base_url.clone();
            endpoint.set_path("/api/v1/channel-relay/reply");
            let mut response = self
                .http
                .post(endpoint)
                .bearer_auth(secret.expose())
                .json(&serde_json::json!({"message_id":message_id,"reply":{"text":text}}))
                .send()
                .await
                .map_err(|_| DeliveryFailure::ResponseLost)?;
            // Pinned NyxID consumes tokens before validation and stores replies
            // after platform IO. No HTTP failure status proves absence of send.
            if response.status() != reqwest::StatusCode::OK {
                return Err(DeliveryFailure::ProviderFailure);
            }
            if response.content_length().is_some_and(|size| size > 4096) {
                return Err(DeliveryFailure::ReceiptInvalid);
            }
            let mut bytes = Zeroizing::new(Vec::new());
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| DeliveryFailure::ResponseLost)?
            {
                if bytes.len().saturating_add(chunk.len()) > 4096 {
                    return Err(DeliveryFailure::ReceiptInvalid);
                }
                bytes.extend_from_slice(&chunk);
            }
            let response: ReplyResponse =
                serde_json::from_slice(&bytes).map_err(|_| DeliveryFailure::ReceiptInvalid)?;
            if !canonical_id(&response.message_id)
                || response.message_id == message_id
                || response
                    .platform_message_id
                    .as_ref()
                    .is_some_and(|id| !canonical_id(id))
            {
                return Err(DeliveryFailure::ReceiptInvalid);
            }
            Ok(DeliveryReceipt {
                nyxid_message_id: response.message_id,
                platform_message_id: response.platform_message_id,
            })
        })
        .await;
        match result {
            Ok(Ok(receipt)) => DeliveryOutcome::Confirmed(receipt),
            Ok(Err(reason)) => DeliveryOutcome::Unknown(reason),
            Err(_) => DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost),
        }
    }
}

#[cfg(test)]
mod tests;
