//! NyxID relay transport. A lost response never proves that nothing was sent.
use super::intents::DeliveryOutcome;
use cmaeg_core::{channel::Secret, seal::ReplyAddress};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use std::time::Duration;

pub enum Credential<'a> {
    ReplyToken(&'a Secret),
    ChannelKey(&'a Secret),
}
#[derive(Clone)]
pub struct RelayClient {
    http: Client,
    endpoint: Url,
    timeout: Duration,
    pub telegram: crate::telegram::TelegramClient,
    pub interactions: Option<std::sync::Arc<crate::interactions::InteractionStore>>,
}
#[derive(Deserialize)]
struct ReplyResponse {
    message_id: String,
    platform_message_id: Option<String>,
}
impl RelayClient {
    pub fn with_telegram(mut self, client: crate::telegram::TelegramClient) -> Self {
        self.telegram = client;
        self
    }
    pub fn with_interactions(
        mut self,
        store: std::sync::Arc<crate::interactions::InteractionStore>,
    ) -> Self {
        self.interactions = Some(store);
        self
    }
    pub fn new(base_url: &str, timeout: Duration) -> Result<Self, crate::ConfigError> {
        let origin = crate::config::validate_origin(crate::CMAEG_NYXID_BASE_URL, base_url)?;
        let endpoint =
            Url::parse(&format!("{origin}/api/v1/channel-relay/reply")).map_err(|_| {
                crate::ConfigError::InvalidManagement {
                    detail: "invalid relay origin".into(),
                }
            })?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .build()
            .map_err(|_| crate::ConfigError::InvalidManagement {
                detail: "cannot build relay client".into(),
            })?;
        Ok(Self {
            http,
            endpoint,
            timeout,
            interactions: None,
            telegram: crate::telegram::TelegramClient::production(timeout)?,
        })
    }
    pub async fn send(
        &self,
        address: &ReplyAddress,
        credential: Credential<'_>,
        text: &str,
    ) -> DeliveryOutcome {
        self.send_rich(address, credential, text, None).await.0
    }
    pub async fn send_rich(
        &self,
        address: &ReplyAddress,
        credential: Credential<'_>,
        text: &str,
        markup: Option<&serde_json::Value>,
    ) -> (DeliveryOutcome, Option<String>) {
        let ReplyAddress::NyxidRelay { message_id, .. } = address else {
            return (DeliveryOutcome::Unsupported, None);
        };
        let (Credential::ReplyToken(secret) | Credential::ChannelKey(secret)) = credential;
        if secret.as_str().is_empty() || !secret.as_str().bytes().all(|b| (33..=126).contains(&b)) {
            return (
                DeliveryOutcome::Rejected {
                    reason: "no_credential".into(),
                },
                None,
            );
        }
        tokio::time::timeout(self.timeout, self.post(message_id, secret, text, markup))
            .await
            .unwrap_or_else(|_| (unknown("response_lost"), None))
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
        let result=self.http.post(endpoint).bearer_auth(secret.as_str()).timeout(self.timeout).json(&serde_json::json!({"message_id":destination.platform_id,"outbound_message_id":destination.outbound_id,"reply":{"text":text,"metadata":{"reply_markup":markup}}})).send().await;
        match result {
            Ok(response) if response.status().is_success() => DeliveryOutcome::Confirmed {
                platform_message_id: Some(destination.platform_id.clone()),
            },
            Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS => {
                DeliveryOutcome::RateLimited {
                    retry_after: retry_after(response.headers()),
                }
            }
            _ => unknown("edit_unconfirmed"),
        }
    }
    async fn post(
        &self,
        message_id: &str,
        secret: &Secret,
        text: &str,
        markup: Option<&serde_json::Value>,
    ) -> (DeliveryOutcome, Option<String>) {
        let mut response = match self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(secret.as_str())
            .json(&serde_json::json!({"message_id":message_id,"reply":{"text":text,"metadata":markup.map(|value|serde_json::json!({"reply_markup":value}))}}))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return (unknown("response_lost"),None),
        };
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return (
                DeliveryOutcome::RateLimited {
                    retry_after: retry_after(response.headers()),
                },
                None,
            );
        }
        if response.status() != StatusCode::OK {
            return (unknown("provider_failure"), None);
        }
        if response
            .content_length()
            .is_some_and(|length| length > 4096)
        {
            return (unknown("receipt_invalid"), None);
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= 4096 => {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                Ok(Some(_)) => return (unknown("receipt_invalid"), None),
                Err(_) => return (unknown("response_lost"), None),
            }
        }
        let Ok(receipt) = serde_json::from_slice::<ReplyResponse>(&bytes) else {
            return (unknown("receipt_invalid"), None);
        };
        if !valid_id(&receipt.message_id)
            || receipt.message_id == message_id
            || receipt
                .platform_message_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
        {
            return (unknown("receipt_invalid"), None);
        }
        (
            DeliveryOutcome::Confirmed {
                platform_message_id: receipt.platform_message_id,
            },
            Some(receipt.message_id),
        )
    }
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn retry_after(headers: &reqwest::header::HeaderMap) -> u64 {
    let Some(raw) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
    else {
        return 1;
    };
    if let Ok(seconds) = raw.parse::<u64>() {
        return seconds.clamp(1, 10);
    }
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc2822)
        .ok()
        .map(|at| {
            (at.unix_timestamp() - time::OffsetDateTime::now_utc().unix_timestamp()).clamp(1, 10)
                as u64
        })
        .unwrap_or(1)
}
pub(super) fn unknown(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Unknown {
        reason: reason.into(),
    }
}
