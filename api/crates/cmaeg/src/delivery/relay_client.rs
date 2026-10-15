//! NyxID relay transport. A lost response never proves that nothing was sent.
use super::intents::DeliveryOutcome;
use crate::net::exchange::{BodyError, HttpReply, TransportError};
use cmaeg_core::{channel::Secret, seal::ReplyAddress};
use futures_util::future::BoxFuture;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Client, StatusCode, Url,
};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

/// The largest relay receipt the client reads.
const RECEIPT_MAX_BYTES: usize = 4096;
/// Complete ordinary delivery's serialized request limit, including identifiers,
/// metadata and JSON escaping. Automatic output is bounded at 256 KiB and sanitization plus JSON
/// expands at most fourfold; tool requests arrive within 128 KiB. Two MiB
/// accommodates both with envelope headroom, independently of platform limits.
/// Low-level `send` and `send_rich` do not impose this limit.
pub const REQUEST_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Serialized before a sender commits or claims a single-use credential.
pub(super) struct PreparedReply {
    body: Vec<u8>,
    message_id: String,
}

pub enum Credential<'a> {
    ReplyToken(&'a Secret),
    ChannelKey(&'a Secret),
}
/// Replaceable IO boundary of the relay client, mirroring `WebhookTransport`:
/// URL building, status mapping, `Retry-After` and receipt parsing stay in the
/// client. An implementation honours `timeout` for the whole exchange.
pub trait RelayTransport: Send + Sync {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>>;
}
/// The production relay transport: one reqwest request per call.
pub struct HttpTransport {
    http: Client,
}
impl HttpTransport {
    fn new(timeout: Duration) -> Result<Self, crate::ConfigError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .build()
            .map_err(|_| crate::ConfigError::InvalidManagement {
                detail: "cannot build relay client".into(),
            })?;
        Ok(Self { http })
    }
}
impl RelayTransport for HttpTransport {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            let response = self
                .http
                .post(url)
                .headers(headers)
                .body(body)
                .timeout(timeout)
                .send()
                .await
                .map_err(|_| TransportError::Unavailable)?;
            let status = response.status();
            let headers = response.headers().clone();
            // Only a 200 answer carries a receipt. Every other answer is mapped
            // from its status alone, so its body is never read.
            let body = if status == StatusCode::OK {
                crate::net::exchange::read_bounded(response, RECEIPT_MAX_BYTES).await
            } else {
                Ok(Vec::new())
            };
            Ok(HttpReply {
                status,
                headers,
                body,
            })
        })
    }
}
#[derive(Clone)]
pub struct RelayClient {
    transport: Arc<dyn RelayTransport>,
    endpoint: Url,
    timeout: Duration,
    complete_replies: bool,
    pub telegram: crate::telegram::TelegramClient,
    pub interactions: Option<std::sync::Arc<crate::interactions::InteractionStore>>,
}
#[derive(Deserialize)]
struct ReplyResponse {
    message_id: String,
    platform_message_id: Option<String>,
}
impl RelayClient {
    /// Select complete ordinary replies in the delivery engine. Enabling this
    /// attests that NyxID splits ordinary/thread replies without truncation and
    /// acknowledges only all-parts acceptance. Low-level sends keep their
    /// existing payload behavior regardless of this setting.
    pub fn with_complete_replies(mut self, enabled: bool) -> Self {
        self.complete_replies = enabled;
        self
    }
    pub(super) fn complete_replies(&self) -> bool {
        self.complete_replies
    }
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
    /// Send through `transport` instead of HTTP, as tests answering in process do.
    pub fn with_transport(mut self, transport: Arc<dyn RelayTransport>) -> Self {
        self.transport = transport;
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
        Ok(Self {
            transport: Arc::new(HttpTransport::new(timeout)?),
            endpoint,
            timeout,
            complete_replies: false,
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
        let secret = match credential_secret(credential) {
            Ok(secret) => secret,
            Err(outcome) => return (outcome, None),
        };
        tokio::time::timeout(self.timeout, self.post(message_id, secret, text, markup))
            .await
            .unwrap_or_else(|_| (unknown("response_lost"), None))
    }
    pub(super) fn prepare(
        address: &ReplyAddress,
        text: &str,
        markup: Option<&serde_json::Value>,
    ) -> Result<PreparedReply, DeliveryOutcome> {
        let ReplyAddress::NyxidRelay { message_id, .. } = address else {
            return Err(DeliveryOutcome::Unsupported);
        };
        let body =
            serialize_reply(message_id, text, markup).map_err(|_| DeliveryOutcome::Rejected {
                reason: "relay_request_invalid".into(),
            })?;
        if body.len() > REQUEST_MAX_BYTES {
            return Err(DeliveryOutcome::Rejected {
                reason: "relay_request_too_large".into(),
            });
        }
        Ok(PreparedReply {
            body,
            message_id: message_id.clone(),
        })
    }
    pub(super) async fn send_prepared(
        &self,
        credential: Credential<'_>,
        prepared: &PreparedReply,
    ) -> (DeliveryOutcome, Option<String>) {
        let secret = match credential_secret(credential) {
            Ok(secret) => secret,
            Err(outcome) => return (outcome, None),
        };
        tokio::time::timeout(self.timeout, self.post_prepared(secret, prepared))
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
    async fn post(
        &self,
        message_id: &str,
        secret: &Secret,
        text: &str,
        markup: Option<&serde_json::Value>,
    ) -> (DeliveryOutcome, Option<String>) {
        let Some(headers) = headers(secret) else {
            return (unknown("response_lost"), None);
        };
        let Ok(body) = serialize_reply(message_id, text, markup) else {
            return (unknown("response_lost"), None);
        };
        self.post_serialized(message_id, headers, body).await
    }
    async fn post_prepared(
        &self,
        secret: &Secret,
        prepared: &PreparedReply,
    ) -> (DeliveryOutcome, Option<String>) {
        let Some(headers) = headers(secret) else {
            return (unknown("response_lost"), None);
        };
        self.post_serialized(&prepared.message_id, headers, prepared.body.clone())
            .await
    }
    async fn post_serialized(
        &self,
        message_id: &str,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> (DeliveryOutcome, Option<String>) {
        let reply = match self
            .transport
            .post(self.endpoint.clone(), headers, body, self.timeout)
            .await
        {
            Ok(reply) => reply,
            Err(_) => return (unknown("response_lost"), None),
        };
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            return (
                DeliveryOutcome::RateLimited {
                    retry_after: retry_after(&reply.headers),
                },
                None,
            );
        }
        if reply.status != StatusCode::OK {
            return (unknown("provider_failure"), None);
        }
        let bytes = match reply.body {
            Ok(bytes) => bytes,
            Err(BodyError::Oversized) => return (unknown("receipt_invalid"), None),
            Err(BodyError::Lost) => return (unknown("response_lost"), None),
        };
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
fn credential_secret(credential: Credential<'_>) -> Result<&Secret, DeliveryOutcome> {
    let (Credential::ReplyToken(secret) | Credential::ChannelKey(secret)) = credential;
    if secret.as_str().is_empty() || !secret.as_str().bytes().all(|b| (33..=126).contains(&b)) {
        return Err(DeliveryOutcome::Rejected {
            reason: "no_credential".into(),
        });
    }
    Ok(secret)
}
fn serialize_reply(
    message_id: &str,
    text: &str,
    markup: Option<&serde_json::Value>,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(
        &serde_json::json!({"message_id":message_id,"reply":{"text":text,"metadata":markup.map(|value|serde_json::json!({"reply_markup":value}))}}),
    )
}
/// The bearer and JSON headers reqwest's `bearer_auth` and `json` set, with the
/// credential marked sensitive. `None` when the credential is not a header value.
fn headers(secret: &Secret) -> Option<HeaderMap> {
    let mut authorization = HeaderValue::try_from(format!("Bearer {}", secret.as_str())).ok()?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Some(headers)
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn retry_after(headers: &HeaderMap) -> u64 {
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
