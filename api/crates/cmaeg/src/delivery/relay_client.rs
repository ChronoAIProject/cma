//! NyxID relay transport. A lost response never proves that nothing was sent.
#[path = "relay_wire.rs"]
mod wire;
pub(super) use wire::unknown;
use wire::{credential_secret, headers, retry_after, serialize_reply, valid_id};
mod discord;
mod initiated;
mod media;
pub use media::supported as native_media_supported;
mod edit;
mod lark;
mod transport;
mod whatsapp;
pub use transport::{HttpTransport, RelayTransport};

use super::intents::DeliveryOutcome;
use crate::net::exchange::BodyError;
use cmaeg_core::{channel::Secret, seal::ReplyAddress};
use reqwest::{header::HeaderMap, StatusCode, Url};
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
    commonmark: bool,
}

pub enum Credential<'a> {
    ReplyToken(&'a Secret),
    ChannelKey(&'a Secret),
}
#[derive(Clone)]
pub struct RelayClient {
    transport: Arc<dyn RelayTransport>,
    endpoint: Url,
    timeout: Duration,
    complete_replies: bool,
    commonmark_relay: crate::config::CommonmarkRelay,
    native_media: bool,
    pub(super) native_lark_outbound_media: bool,
    pub(super) native_lark_sticker_replies: bool,
    pub(super) native_lark_effects: bool,
    pub(super) native_split_text_refs: bool,
    pub(super) telegram_media_edits: bool,
    pub(super) discord_media_edits: bool,
    pub native_media_receipts: Option<Arc<crate::native_media::store::Store>>,
    pub(super) native_lark_controls: bool,
    pub(super) native_lark_media_controls: bool,
    pub(super) native_lark_multi_image_controls: bool,
    pub(super) native_lark_media_card_replacements: bool,
    pub(super) native_media_presentations: bool,
    pub(super) native_discord_voice: bool,
    pub(super) discord_voice_decoder: Option<crate::discord::voice::Decoder>,
    pub(super) native_telegram_structured_replies: bool,
    pub(super) native_whatsapp_structured_replies: bool,
    pub(super) native_telegram_albums: bool,
    pub(super) native_lark_thread_start: bool,
    pub(super) native_lark_forms: bool,
    native_whatsapp_outbound_media: bool,
    native_whatsapp_templates: bool,
    native_whatsapp_interactions: bool,
    native_whatsapp_media_controls: bool,
    native_whatsapp_groups: bool,
    text_edits: bool,
    native_discord_edits: bool,
    native_discord_effects: bool,
    native_discord_media_effects: bool,
    lark_cards: bool,
    lark_forms: bool,
    whatsapp_templates: bool,
    whatsapp_interactions: bool,
    discord_interactions: bool,
    discord_media_controls: bool,
    discord_media_card_edits: bool,
    discord_media_card_replacements: bool,
    pub(super) discord_media_card_sets: bool,
    discord_reactions: bool,
    discord_forms: bool,
    discord_multiselect: bool,
    discord_card_edits: bool,
    consumer_interactions: bool,
    telegram_effects: bool,
    telegram_media_effects: bool,
    telegram_media_controls: bool,
    pub(super) telegram_media_control_edits: bool,
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

    /// Issue edits only after every reader understands retained media snapshots and claims.
    pub fn with_telegram_media_control_edits(mut self, enabled: bool) -> Self {
        self.telegram_media_control_edits = enabled;
        self
    }
    /// Enable new media cards only after every reader understands the media marker.
    pub fn with_telegram_media_controls(mut self, enabled: bool) -> Self {
        self.telegram_media_controls = enabled;
        self
    }
    pub(super) fn telegram_media_controls_enabled(&self) -> bool {
        self.telegram_media_controls
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
    /// Issue enriched text references only after every gateway reader supports them.
    /// Retained references remain editable when issuance is disabled again.
    pub fn with_text_edits(mut self, enabled: bool) -> Self {
        self.text_edits = enabled;
        self
    }
    pub(super) fn text_edits_enabled(&self) -> bool {
        self.text_edits
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
    /// Enable only after every serving NyxID replica supports the explicit routes.
    pub fn with_commonmark_relay(mut self, enabled: bool) -> Self {
        self.commonmark_relay.telegram = enabled;
        self
    }
    pub(crate) fn with_commonmark_platforms(
        mut self,
        platforms: crate::config::CommonmarkRelay,
    ) -> Self {
        self.commonmark_relay = platforms;
        self
    }
    pub(super) fn commonmark_for(&self, address: &ReplyAddress) -> bool {
        matches!(address, ReplyAddress::NyxidRelay { platform, .. }
            if self.commonmark_for_platform(*platform))
    }
    pub(super) fn commonmark_for_platform(&self, platform: cmaeg_core::channel::Platform) -> bool {
        self.commonmark_relay.enabled_for(platform)
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
            commonmark_relay: Default::default(),
            native_media: false,
            native_lark_outbound_media: false,
            native_lark_sticker_replies: false,
            native_lark_effects: false,
            native_split_text_refs: false,
            telegram_media_edits: false,
            discord_media_edits: false,
            native_media_receipts: None,
            native_lark_controls: false,
            native_lark_media_controls: false,
            native_lark_multi_image_controls: false,
            native_lark_media_card_replacements: false,
            native_media_presentations: false,
            native_discord_voice: false,
            discord_voice_decoder: None,
            native_telegram_structured_replies: false,
            native_whatsapp_structured_replies: false,
            native_telegram_albums: false,
            native_lark_thread_start: false,
            native_lark_forms: false,
            native_whatsapp_outbound_media: false,
            native_whatsapp_templates: false,
            native_whatsapp_interactions: false,
            native_whatsapp_media_controls: false,
            native_whatsapp_groups: false,
            text_edits: false,
            native_discord_edits: false,
            native_discord_effects: false,
            native_discord_media_effects: false,
            lark_cards: false,
            lark_forms: false,
            whatsapp_templates: false,
            whatsapp_interactions: false,
            discord_interactions: false,
            discord_media_controls: false,
            discord_media_card_edits: false,
            discord_media_card_replacements: false,
            discord_media_card_sets: false,
            discord_reactions: false,
            discord_forms: false,
            discord_multiselect: false,
            discord_card_edits: false,
            consumer_interactions: false,
            telegram_effects: false,
            telegram_media_effects: false,
            telegram_media_controls: false,
            telegram_media_control_edits: false,
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
            self.post(
                message_id,
                secret,
                text,
                markup,
                *thread_reply,
                self.commonmark_for(address),
            ),
        )
        .await
        .unwrap_or_else(|_| (unknown("response_lost"), None))
    }
    pub(super) fn prepare(
        address: &ReplyAddress,
        text: &str,
        markup: Option<&serde_json::Value>,
        commonmark: bool,
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
            commonmark,
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
        } else if (self.complete_replies || prepared.commonmark) && prepared.complete_text {
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
        commonmark: bool,
    ) -> (DeliveryOutcome, Option<String>) {
        let Some(headers) = headers(secret) else {
            return (unknown("response_lost"), None);
        };
        let Ok(body) = serialize_reply(message_id, text, markup, thread_reply) else {
            return (unknown("response_lost"), None);
        };
        self.post_serialized(
            message_id,
            headers,
            body,
            thread_reply,
            commonmark,
            self.timeout,
        )
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
            prepared.commonmark,
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
        commonmark: bool,
        timeout: Duration,
    ) -> (DeliveryOutcome, Option<String>) {
        let mut endpoint = self.endpoint.clone();
        if thread_reply {
            // Old NyxID replicas must fail before sending, never ignore the flag.
            endpoint.set_path("/api/v1/channel-relay/thread-reply");
        }
        if commonmark {
            endpoint.set_path(if thread_reply {
                "/api/v1/channel-relay/thread-reply/commonmark"
            } else {
                "/api/v1/channel-relay/reply/commonmark"
            });
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
