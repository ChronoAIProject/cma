//! NyxID relay transport. A lost response never proves that nothing was sent.
mod discord;
mod initiated;
mod media;
pub use media::supported as native_media_supported;
mod edit;
mod lark;
mod whatsapp;

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
/// Complete ordinary delivery limit, including identifiers, metadata and JSON escaping.
/// Automatic output is bounded at 256 KiB with at most fourfold expansion; tools at
/// 128 KiB. Two MiB accommodates both with envelope headroom, independently of platform limits.
/// Low-level `send` and `send_rich` do not impose this limit.
pub const REQUEST_MAX_BYTES: usize = 2 * 1024 * 1024;
/// NyxID sends a complete ordinary reply's parts sequentially. Leave room for
/// queueing/pacing inside the existing 20 s routed delivery window.
pub(crate) const COMPLETE_REPLY_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const COMPLETE_REPLY_TOOL_TIMEOUT: Duration = Duration::from_secs(18);

/// Serialized before a sender commits or claims a single-use credential.
pub(super) struct PreparedReply {
    body: Vec<u8>,
    message_id: String,
    thread_reply: bool,
    native_media: bool,
    complete_text: bool,
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
    native_media: bool,
    native_whatsapp_outbound_media: bool,
    text_edits: bool,
    native_discord_edits: bool,
    native_discord_effects: bool,
    native_discord_media_effects: bool,
    lark_cards: bool,
    lark_forms: bool,
    whatsapp_templates: bool,
    whatsapp_interactions: bool,
    discord_interactions: bool,
    discord_reactions: bool,
    discord_forms: bool,
    discord_multiselect: bool,
    discord_card_edits: bool,
    consumer_interactions: bool,
    telegram_effects: bool,
    telegram_media_effects: bool,
    pub native_lark: crate::native_lark::Client,
    pub native_lark_receipts: Option<Arc<crate::native_lark::receipts::Store>>,
    pub whatsapp: crate::whatsapp::Client,
    pub whatsapp_receipts: Option<Arc<crate::whatsapp::receipts::Store>>,
    pub telegram: crate::telegram::TelegramClient,
    pub discord: crate::discord::DiscordRest,
    pub interactions: Option<std::sync::Arc<crate::interactions::InteractionStore>>,
}
#[derive(Deserialize)]
struct ReplyResponse {
    message_id: String,
    platform_message_id: Option<String>,
}
impl RelayClient {
    /// Gate named-consumer cards until all readers support retained owner bindings.
    pub fn with_consumer_interactions(mut self, enabled: bool) -> Self {
        self.consumer_interactions = enabled;
        self
    }
    pub(super) fn consumer_interactions_enabled(&self) -> bool {
        self.consumer_interactions
    }

    pub fn with_telegram_effects(mut self, enabled: bool) -> Self {
        self.telegram_effects = enabled;
        self
    }
    pub(super) fn telegram_effects_enabled(&self) -> bool {
        self.telegram_effects
    }
    /// New media scopes require compatible readers; retained effects stay usable.
    pub fn with_telegram_media_effects(mut self, enabled: bool) -> Self {
        self.telegram_media_effects = enabled;
        self
    }
    pub(super) fn telegram_media_effects_enabled(&self) -> bool {
        self.telegram_media_effects
    }
    /// Gate new card issuance until every reader understands retained Lark cards.
    pub fn with_lark_cards(mut self, enabled: bool) -> Self {
        self.lark_cards = enabled;
        self
    }
    pub(super) fn lark_cards_enabled(&self) -> bool {
        self.lark_cards
    }
    /// Issue enriched text references only after every gateway reader supports them.
    /// Retained references remain editable when issuance is disabled again.
    pub fn with_text_edits(mut self, enabled: bool) -> Self {
        self.text_edits = enabled;
        self
    }
    pub(super) fn text_edits_enabled(&self) -> bool {
        self.text_edits
    }
    /// Attest that every deployed NyxID replica honors bounded native media replies.
    pub fn with_native_media(mut self, enabled: bool) -> Self {
        self.native_media = enabled;
        self
    }
    pub(crate) fn native_media_enabled(&self) -> bool {
        self.native_media
    }
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
            native_media: false,
            native_whatsapp_outbound_media: false,
            text_edits: false,
            native_discord_edits: false,
            native_discord_effects: false,
            native_discord_media_effects: false,
            lark_cards: false,
            lark_forms: false,
            whatsapp_templates: false,
            whatsapp_interactions: false,
            discord_interactions: false,
            discord_reactions: false,
            discord_forms: false,
            discord_multiselect: false,
            discord_card_edits: false,
            consumer_interactions: false,
            telegram_effects: false,
            telegram_media_effects: false,
            interactions: None,
            native_lark: crate::native_lark::Client::production()?,
            native_lark_receipts: None,
            whatsapp: crate::whatsapp::Client::production()?,
            whatsapp_receipts: None,
            telegram: crate::telegram::TelegramClient::production(timeout)?,
            discord: crate::discord::DiscordRest::new()?,
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
        let ReplyAddress::NyxidRelay {
            message_id,
            thread_reply,
            ..
        } = address
        else {
            return (DeliveryOutcome::Unsupported, None);
        };
        let secret = match credential_secret(credential) {
            Ok(secret) => secret,
            Err(outcome) => return (outcome, None),
        };
        tokio::time::timeout(
            self.timeout,
            self.post(message_id, secret, text, markup, *thread_reply),
        )
        .await
        .unwrap_or_else(|_| (unknown("response_lost"), None))
    }
    pub(super) fn prepare(
        address: &ReplyAddress,
        text: &str,
        markup: Option<&serde_json::Value>,
    ) -> Result<PreparedReply, DeliveryOutcome> {
        let ReplyAddress::NyxidRelay {
            message_id,
            thread_reply,
            ..
        } = address
        else {
            return Err(DeliveryOutcome::Unsupported);
        };
        let body = serialize_reply(message_id, text, markup, *thread_reply).map_err(|_| {
            DeliveryOutcome::Rejected {
                reason: "relay_request_invalid".into(),
            }
        })?;
        if body.len() > REQUEST_MAX_BYTES {
            return Err(DeliveryOutcome::Rejected {
                reason: "relay_request_too_large".into(),
            });
        }
        Ok(PreparedReply {
            body,
            message_id: message_id.clone(),
            thread_reply: *thread_reply,
            native_media: false,
            complete_text: true,
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
        let timeout = if prepared.native_media {
            Duration::from_secs(60)
        } else if self.complete_replies && prepared.complete_text {
            COMPLETE_REPLY_TIMEOUT
        } else {
            self.timeout
        };
        let result = tokio::time::timeout(timeout, self.post_prepared(secret, prepared, timeout))
            .await
            .unwrap_or_else(|_| (unknown("response_lost"), None));
        if prepared.native_media
            && matches!(
                result.0,
                DeliveryOutcome::Confirmed {
                    platform_message_id: None
                }
            )
        {
            (unknown("receipt_invalid"), None)
        } else {
            result
        }
    }
    async fn post(
        &self,
        message_id: &str,
        secret: &Secret,
        text: &str,
        markup: Option<&serde_json::Value>,
        thread_reply: bool,
    ) -> (DeliveryOutcome, Option<String>) {
        let Some(headers) = headers(secret) else {
            return (unknown("response_lost"), None);
        };
        let Ok(body) = serialize_reply(message_id, text, markup, thread_reply) else {
            return (unknown("response_lost"), None);
        };
        self.post_serialized(message_id, headers, body, thread_reply, self.timeout)
            .await
    }
    async fn post_prepared(
        &self,
        secret: &Secret,
        prepared: &PreparedReply,
        timeout: Duration,
    ) -> (DeliveryOutcome, Option<String>) {
        let Some(headers) = headers(secret) else {
            return (unknown("response_lost"), None);
        };
        self.post_serialized(
            &prepared.message_id,
            headers,
            prepared.body.clone(),
            prepared.thread_reply,
            timeout,
        )
        .await
    }
    async fn post_serialized(
        &self,
        message_id: &str,
        headers: HeaderMap,
        body: Vec<u8>,
        thread_reply: bool,
        timeout: Duration,
    ) -> (DeliveryOutcome, Option<String>) {
        let mut endpoint = self.endpoint.clone();
        if thread_reply {
            // Old NyxID replicas must fail before sending, never ignore the flag.
            endpoint.set_path("/api/v1/channel-relay/thread-reply");
        }
        let reply = match self.transport.post(endpoint, headers, body, timeout).await {
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
    thread_reply: bool,
) -> Result<Vec<u8>, serde_json::Error> {
    let mut body = serde_json::json!({"message_id":message_id,"reply":{"text":text,"metadata":markup.map(|value|serde_json::json!({"reply_markup":value}))}});
    if thread_reply {
        body["thread_reply"] = true.into();
    }
    serde_json::to_vec(&body)
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
