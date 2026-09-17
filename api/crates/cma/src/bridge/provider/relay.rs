use layer2_core::bridge::outbox::{
    DeliveryFailure, DeliveryOutcome, DeliveryReceipt, RelayDeliveryRequest, RelayOperation,
};
use serde::Deserialize;

use super::*;
mod retry_after;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyResponse {
    message_id: String,
    platform_message_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditResponse {
    upstream_message_id: String,
    edited_at: String,
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
        match &request.item().operation {
            RelayOperation::InitialReply => {
                self.post_reply(
                    request.item().source.message_id(),
                    request.item().text(),
                    secret,
                    request.deadline(),
                )
                .await
            }
            RelayOperation::Edit { .. } => {
                if !layer2_core::bridge::outbox::editable_platform(&connector.platform) {
                    return DeliveryOutcome::DefinitiveRejection(
                        DeliveryFailure::CapabilityUnsupported,
                    );
                }
                let Some(original) = request.edit_original_receipt() else {
                    return DeliveryOutcome::DefinitiveRejection(DeliveryFailure::ReceiptInvalid);
                };
                self.post_edit(original, request.item().text(), secret, request.deadline())
                    .await
            }
        }
    }

    async fn post_reply(
        &self,
        message_id: &str,
        text: &str,
        secret: &SecretValue,
        deadline: tokio::time::Instant,
    ) -> DeliveryOutcome {
        let bytes = match self
            .post(
                "/api/v1/channel-relay/reply",
                message_id,
                text,
                secret,
                deadline,
            )
            .await
        {
            Ok(bytes) => bytes,
            Err(outcome) => return outcome,
        };
        let Ok(response) = serde_json::from_slice::<ReplyResponse>(&bytes) else {
            return DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid);
        };
        if !canonical_id(&response.message_id)
            || response.message_id == message_id
            || response
                .platform_message_id
                .as_ref()
                .is_some_and(|id| !canonical_id(id))
        {
            return DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid);
        }
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: response.message_id,
            platform_message_id: response.platform_message_id,
        })
    }

    async fn post_edit(
        &self,
        original: &DeliveryReceipt,
        text: &str,
        secret: &SecretValue,
        deadline: tokio::time::Instant,
    ) -> DeliveryOutcome {
        let Some(platform_id) = original
            .platform_message_id
            .as_deref()
            .filter(|id| canonical_id(id))
        else {
            return DeliveryOutcome::DefinitiveRejection(DeliveryFailure::ReceiptInvalid);
        };
        let bytes = match self
            .post(
                "/api/v1/channel-relay/reply/update",
                platform_id,
                text,
                secret,
                deadline,
            )
            .await
        {
            Ok(bytes) => bytes,
            Err(outcome) => return outcome,
        };
        let Ok(response) = serde_json::from_slice::<EditResponse>(&bytes) else {
            return DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid);
        };
        if response.upstream_message_id != platform_id
            || layer2_core::bridge::provider_timestamp(&response.edited_at).is_none()
        {
            return DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid);
        }
        // The update acknowledges this existing message; it creates no new
        // NyxID message ID. Keep the original exact confirmed receipt identity.
        DeliveryOutcome::Confirmed(original.clone())
    }

    async fn post(
        &self,
        path: &str,
        message_id: &str,
        text: &str,
        secret: &SecretValue,
        deadline: tokio::time::Instant,
    ) -> Result<Zeroizing<Vec<u8>>, DeliveryOutcome> {
        tokio::time::timeout_at(deadline, async {
            let mut endpoint = self.base_url.clone();
            endpoint.set_path(path);
            let mut response = self
                .http
                .post(endpoint)
                .bearer_auth(secret.expose())
                .json(&serde_json::json!({"message_id":message_id,"reply":{"text":text}}))
                .send()
                .await
                .map_err(|_| DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost))?;
            // HTTP failure after request dispatch does not prove absence of send/edit.
            if response.status() != reqwest::StatusCode::OK {
                return Err(
                    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        DeliveryOutcome::RateLimitedUnknown {
                            not_before_ms: retry_after::not_before(
                                response.headers(),
                                std::time::SystemTime::now(),
                            ),
                        }
                    } else {
                        DeliveryOutcome::Unknown(DeliveryFailure::ProviderFailure)
                    },
                );
            }
            if response.content_length().is_some_and(|size| size > 4096) {
                return Err(DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid));
            }
            let mut bytes = Zeroizing::new(Vec::new());
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost))?
            {
                if bytes.len().saturating_add(chunk.len()) > 4096 {
                    return Err(DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost))?
    }
}

#[cfg(test)]
mod tests;
