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
}
#[derive(Deserialize)]
struct ReplyResponse {
    message_id: String,
    platform_message_id: Option<String>,
}
impl RelayClient {
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
        })
    }
    pub async fn send(
        &self,
        address: &ReplyAddress,
        credential: Credential<'_>,
        text: &str,
    ) -> DeliveryOutcome {
        let ReplyAddress::NyxidRelay { message_id, .. } = address else {
            return DeliveryOutcome::Unsupported;
        };
        let (Credential::ReplyToken(secret) | Credential::ChannelKey(secret)) = credential;
        if secret.as_str().is_empty() || !secret.as_str().bytes().all(|b| (33..=126).contains(&b)) {
            return DeliveryOutcome::Rejected {
                reason: "no_credential".into(),
            };
        }
        tokio::time::timeout(self.timeout, self.post(message_id, secret, text))
            .await
            .unwrap_or_else(|_| unknown("response_lost"))
    }
    async fn post(&self, message_id: &str, secret: &Secret, text: &str) -> DeliveryOutcome {
        let mut response = match self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(secret.as_str())
            .json(&serde_json::json!({"message_id":message_id,"reply":{"text":text}}))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return unknown("response_lost"),
        };
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return DeliveryOutcome::RateLimited {
                retry_after: retry_after(response.headers()),
            };
        }
        if response.status() != StatusCode::OK {
            return unknown("provider_failure");
        }
        if response
            .content_length()
            .is_some_and(|length| length > 4096)
        {
            return unknown("receipt_invalid");
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= 4096 => {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                Ok(Some(_)) => return unknown("receipt_invalid"),
                Err(_) => return unknown("response_lost"),
            }
        }
        let Ok(receipt) = serde_json::from_slice::<ReplyResponse>(&bytes) else {
            return unknown("receipt_invalid");
        };
        if !valid_id(&receipt.message_id)
            || receipt.message_id == message_id
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
