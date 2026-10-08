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

/// The largest relay receipt or conflict body the client reads.
const RECEIPT_MAX_BYTES: usize = 4096;
/// NyxID refused a proactive send: the channel key is not the conversation's
/// agent key, the conversation does not allow agent-initiated messages, or
/// the request was malformed.
pub const DEFAULT_REPLY_REFUSED: &str = "default_reply_refused";
/// The conversation's platform has no initiated send.
pub const DEFAULT_REPLY_UNSUPPORTED: &str = "default_reply_unsupported";
/// An idempotency key was presented for a different message; nothing was sent.
pub const IDEMPOTENCY_CONFLICT: &str = "idempotency_conflict";
/// NyxID holds the key for a send in progress or of uncertain outcome; the
/// message may have been accepted, so it is never retried automatically.
pub const DELIVERY_IN_PROGRESS_OR_UNCERTAIN: &str = "delivery_in_progress_or_uncertain";

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
            // Only a 200 answer carries a receipt, and only a 409's message
            // tells a reused key from a send in progress. Every other answer
            // is mapped from its status alone, so its body is never read.
            let body = if status == StatusCode::OK || status == StatusCode::CONFLICT {
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
    /// NyxID's proactive send, `/api/v1/channel-relay/send`.
    send_endpoint: Url,
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
    /// Send through `transport` instead of HTTP, as tests answering in process do.
    pub fn with_transport(mut self, transport: Arc<dyn RelayTransport>) -> Self {
        self.transport = transport;
        self
    }
    pub fn new(base_url: &str, timeout: Duration) -> Result<Self, crate::ConfigError> {
        let origin = crate::config::validate_origin(crate::CMAEG_NYXID_BASE_URL, base_url)?;
        let url = |path: &str| {
            Url::parse(&format!("{origin}/api/v1/channel-relay/{path}")).map_err(|_| {
                crate::ConfigError::InvalidManagement {
                    detail: "invalid relay origin".into(),
                }
            })
        };
        Ok(Self {
            transport: Arc::new(HttpTransport::new(timeout)?),
            endpoint: url("reply")?,
            send_endpoint: url("send")?,
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
        if !presentable(secret) {
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
        let Ok(body) = serde_json::to_vec(
            &serde_json::json!({"message_id":message_id,"reply":{"text":text,"metadata":markup.map(|value|serde_json::json!({"reply_markup":value}))}}),
        ) else {
            return (unknown("response_lost"), None);
        };
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
        let receipt = match receipt(reply.body) {
            Ok(receipt) => receipt,
            Err(outcome) => return (outcome, None),
        };
        // A reply's receipt names a new message, never the one it answers.
        if receipt.message_id == message_id {
            return (unknown("receipt_invalid"), None);
        }
        (
            DeliveryOutcome::Confirmed {
                platform_message_id: receipt.platform_message_id,
            },
            Some(receipt.message_id),
        )
    }
    /// Send `text` to the relay conversation `route_id` without an inbound
    /// message, through NyxID's proactive send: `POST
    /// {CMAEG_NYXID_BASE_URL}/api/v1/channel-relay/send` with
    /// `{"conversation_id":route_id,"message":{"text":…},"idempotency_key":k}`,
    /// authorized by the channel key. NyxID requires that key to be the
    /// conversation's assigned agent key and the conversation to allow
    /// agent-initiated messages.
    ///
    /// A 429 is returned as `rate_limited` for the caller's single cooldown
    /// retry with the same key. A 409 is never retried here: unless NyxID says
    /// the key was used for a different message, the message may have been
    /// accepted.
    pub async fn send_initiated(
        &self,
        route_id: &str,
        channel_key: &Secret,
        text: &str,
        idempotency_key: &str,
    ) -> DeliveryOutcome {
        if !presentable(channel_key) {
            return DeliveryOutcome::Rejected {
                reason: "no_credential".into(),
            };
        }
        tokio::time::timeout(
            self.timeout,
            self.post_initiated(route_id, channel_key, text, idempotency_key),
        )
        .await
        .unwrap_or_else(|_| unknown("response_lost"))
    }
    async fn post_initiated(
        &self,
        route_id: &str,
        channel_key: &Secret,
        text: &str,
        idempotency_key: &str,
    ) -> DeliveryOutcome {
        let Some(headers) = headers(channel_key) else {
            return unknown("response_lost");
        };
        let Ok(body) = serde_json::to_vec(&serde_json::json!({
            "conversation_id": route_id,
            "message": {"text": text},
            "idempotency_key": idempotency_key,
        })) else {
            return unknown("response_lost");
        };
        let reply = match self
            .transport
            .post(self.send_endpoint.clone(), headers, body, self.timeout)
            .await
        {
            Ok(reply) => reply,
            Err(_) => return unknown("response_lost"),
        };
        let rejected = |reason: &str| DeliveryOutcome::Rejected {
            reason: reason.into(),
        };
        match reply.status {
            StatusCode::OK => match receipt(reply.body) {
                Ok(receipt) => DeliveryOutcome::Confirmed {
                    platform_message_id: receipt.platform_message_id,
                },
                Err(outcome) => outcome,
            },
            StatusCode::TOO_MANY_REQUESTS => DeliveryOutcome::RateLimited {
                retry_after: retry_after(&reply.headers),
            },
            StatusCode::FORBIDDEN => {
                tracing::warn!(status = 403, "initiated relay send refused");
                rejected(DEFAULT_REPLY_REFUSED)
            }
            // The route id is validated as a UUID when it is configured, so
            // NyxID's 400 means the gateway built a request NyxID rejects.
            StatusCode::BAD_REQUEST => {
                tracing::error!(status = 400, "initiated relay send malformed");
                rejected(DEFAULT_REPLY_REFUSED)
            }
            StatusCode::CONFLICT if different_message(&reply.body) => {
                // Unreachable by construction: no key is reused for other text.
                tracing::error!(
                    status = 409,
                    "initiated relay key reused for another message"
                );
                rejected(IDEMPOTENCY_CONFLICT)
            }
            StatusCode::CONFLICT => {
                tracing::warn!(
                    status = 409,
                    "initiated relay send in progress or uncertain"
                );
                unknown(DELIVERY_IN_PROGRESS_OR_UNCERTAIN)
            }
            StatusCode::NOT_IMPLEMENTED => {
                tracing::warn!(status = 501, "initiated relay send unsupported");
                rejected(DEFAULT_REPLY_UNSUPPORTED)
            }
            _ => unknown("provider_failure"),
        }
    }
}
/// A relay receipt: a valid new message id and an optional platform id.
fn receipt(body: Result<Vec<u8>, BodyError>) -> Result<ReplyResponse, DeliveryOutcome> {
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(BodyError::Oversized) => return Err(unknown("receipt_invalid")),
        Err(BodyError::Lost) => return Err(unknown("response_lost")),
    };
    let Ok(receipt) = serde_json::from_slice::<ReplyResponse>(&bytes) else {
        return Err(unknown("receipt_invalid"));
    };
    if !valid_id(&receipt.message_id)
        || receipt
            .platform_message_id
            .as_deref()
            .is_some_and(|id| !valid_id(id))
    {
        return Err(unknown("receipt_invalid"));
    }
    Ok(receipt)
}
/// Whether a 409's body says its key was used for a different message,
/// which NyxID answers without sending anything.
fn different_message(body: &Result<Vec<u8>, BodyError>) -> bool {
    body.as_ref().is_ok_and(|bytes| {
        String::from_utf8_lossy(bytes)
            .to_ascii_lowercase()
            .contains("different message")
    })
}
/// Whether `secret` can be presented as a bearer credential.
fn presentable(secret: &Secret) -> bool {
    !secret.as_str().is_empty() && secret.as_str().bytes().all(|b| (33..=126).contains(&b))
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
