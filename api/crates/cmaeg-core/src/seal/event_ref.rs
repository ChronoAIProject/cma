//! Expiring event reply references sealed as `cmaeg1.ev` tokens.

use std::{fmt, str::FromStr};

use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;

use crate::{
    channel::{validate_https_url, Platform, Secret},
    ids::{ChannelId, ConsumerId, ConversationId, EventId, RecordId},
    require_string,
    seal::{open, seal, Lineage, MasterKeyRing, OpenError, Opened, SealError, TokenKind},
    ValidationError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventRefPayload {
    /// Authenticated native Telegram admission evidence; absent before the reader rollout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_admission: Option<super::TelegramAdmissionProof>,
    /// Optional NyxID turn correlation, diagnostic only and never an authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_trace: Option<TurnTraceContext>,
    pub v: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_sha256: Option<String>,
    pub channel_id: ChannelId,
    pub record_id: RecordId,
    pub event_id: EventId,
    pub partition_key: String,
    pub conversation_id: ConversationId,
    pub expires_at: i64,
    pub reply: ReplyAddress,
    /// The consumer this reference authorizes; absent for the default
    /// consumer (never `Some("default")`), so a default reference keeps the
    /// exact plaintext it had before consumers existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerId>,
    /// Present only on hand-off events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<Lineage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnTraceContext {
    /// Lowercase W3C trace-id bytes, bounded to the admitted upstream turn.
    pub trace_id: String,
    /// The authenticated caller span that led into CMAEG.
    pub parent_span_id: String,
    /// The NyxID request that admitted this callback, when the peer supplied
    /// one. Older callbacks omit it and retain partial trace coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_request_id: Option<String>,
}

impl TurnTraceContext {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.trace_id.len() != 32
            || !self.trace_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.trace_id.bytes().all(|byte| byte == b'0')
        {
            return Err(ValidationError::new(
                "/turn_trace/trace_id",
                "invalid trace id",
            ));
        }
        if self.parent_span_id.len() != 16
            || !self
                .parent_span_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.parent_span_id.bytes().all(|byte| byte == b'0')
        {
            return Err(ValidationError::new(
                "/turn_trace/parent_span_id",
                "invalid parent span id",
            ));
        }
        if self.trace_id.bytes().any(|byte| byte.is_ascii_uppercase())
            || self
                .parent_span_id
                .bytes()
                .any(|byte| byte.is_ascii_uppercase())
        {
            return Err(ValidationError::new(
                "/turn_trace",
                "trace ids must be lowercase",
            ));
        }
        if let Some(request_id) = &self.ingress_request_id {
            let request_id = uuid::Uuid::parse_str(request_id).map_err(|_| {
                ValidationError::new(
                    "/turn_trace/ingress_request_id",
                    "invalid ingress request id",
                )
            })?;
            if request_id.get_version_num() != 4 {
                return Err(ValidationError::new(
                    "/turn_trace/ingress_request_id",
                    "ingress request id must be UUID v4",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, ToSchema)]
pub enum ReplyAddress {
    NativeLark(crate::channel::NativeLarkReply),
    WhatsappGroup(crate::channel::whatsapp::WhatsappGroupReply),
    Whatsapp {
        app_id: String,
        waba_id: String,
        phone_number_id: String,
        recipient_id: String,
        message_id: String,
    },
    Discord {
        bot_id: String,
        channel_id: String,
        message_id: String,
        thread_id: Option<String>,
    },
    Telegram {
        bot_id: String,
        chat_id: String,
        message_id: String,
        thread_id: Option<String>,
    },

    NyxidRelay {
        thread_reply: bool,
        message_id: String,
        route_id: String,
        platform: Platform,
        reply_token: Secret,
        reply_token_expires_at: i64,
    },
    ResultWebhook {
        url: String,
        hmac_key_id: String,
    },
    Sync {
        waiter: String,
    },
    None,
}

impl fmt::Debug for ReplyAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NativeLark(reply) => reply.fmt(formatter),
            Self::WhatsappGroup(reply) => reply.fmt(formatter),
            Self::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            } => formatter
                .debug_struct("Whatsapp")
                .field("app_id", app_id)
                .field("waba_id", waba_id)
                .field("phone_number_id", phone_number_id)
                .field("recipient_id", recipient_id)
                .field("message_id", message_id)
                .finish(),
            Self::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            } => formatter
                .debug_struct("Discord")
                .field("bot_id", bot_id)
                .field("channel_id", channel_id)
                .field("message_id", message_id)
                .field("thread_id", thread_id)
                .finish(),
            Self::NyxidRelay {
                message_id,
                route_id,
                platform,
                reply_token_expires_at,
                ..
            } => formatter
                .debug_struct("NyxidRelay")
                .field("message_id", message_id)
                .field("route_id", route_id)
                .field("platform", platform)
                .field("reply_token", &"[redacted]")
                .field("reply_token_expires_at", reply_token_expires_at)
                .finish(),
            Self::Telegram {
                bot_id,
                chat_id,
                message_id,
                thread_id,
            } => formatter
                .debug_struct("Telegram")
                .field("bot_id", bot_id)
                .field("chat_id", chat_id)
                .field("message_id", message_id)
                .field("thread_id", thread_id)
                .finish(),
            Self::ResultWebhook { url, hmac_key_id } => formatter
                .debug_struct("ResultWebhook")
                .field("url", url)
                .field("hmac_key_id", hmac_key_id)
                .finish(),
            Self::Sync { waiter } => formatter
                .debug_struct("Sync")
                .field("waiter", waiter)
                .finish(),
            Self::None => formatter.write_str("None"),
        }
    }
}

mod wire;

#[derive(Clone, PartialEq, Eq, ToSchema)]
#[schema(value_type = String)]
pub struct EventRef(String);

impl fmt::Debug for EventRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted event ref]")
    }
}

impl EventRef {
    pub fn from_sealed(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn seal(payload: &EventRefPayload, ring: &MasterKeyRing) -> Result<Self, SealError> {
        seal(ring, TokenKind::EventRef, payload).map(Self)
    }

    pub fn open(
        &self,
        ring: &MasterKeyRing,
        now: i64,
    ) -> Result<Opened<EventRefPayload>, OpenError> {
        let opened: Opened<EventRefPayload> = open(ring, TokenKind::EventRef, self.as_str())?;
        opened.payload.validate().map_err(|_| OpenError::Format)?;
        if now >= opened.payload.expires_at {
            return Err(OpenError::Expired);
        }
        Ok(opened)
    }
}

impl FromStr for EventRef {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.starts_with("cmaeg1.ev.") {
            Ok(Self(value.to_owned()))
        } else {
            Err(ValidationError::new("/", "event ref has an invalid prefix"))
        }
    }
}

impl Serialize for EventRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EventRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_str(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

pub fn activity_digest(activity: &crate::activity::Activity) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        crate::profile::canonical_json(
            serde_json::to_value(activity).expect("Activity serialization is infallible"),
        )
        .as_bytes(),
    );
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
/// A reference names only a non-default consumer: the default consumer's
/// references stay exactly as they were before consumers existed (I3).
pub(crate) fn validate_consumer(consumer: Option<&ConsumerId>) -> Result<(), ValidationError> {
    if consumer.is_some_and(ConsumerId::is_default) {
        return Err(ValidationError::new(
            "/consumer",
            "the default consumer is never named in a reference",
        ));
    }
    Ok(())
}

impl EventRefPayload {
    pub fn verifies_activity(&self, activity: &crate::activity::Activity) -> bool {
        self.activity_sha256
            .as_ref()
            .is_some_and(|digest| *digest == activity_digest(activity))
            && self.event_id == activity.event_id
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(trace) = &self.turn_trace {
            trace.validate()?;
        }
        if let Some(proof) = &self.telegram_admission {
            proof.validate()?;
            if self.activity_sha256.is_none()
                || !matches!(self.reply, ReplyAddress::Telegram { .. })
                || self.lineage.is_some()
            {
                return Err(ValidationError::new(
                    "/telegram_admission",
                    "native event required",
                ));
            }
        }
        if self.activity_sha256.as_ref().is_some_and(|d| {
            d.len() != 64
                || !d
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(ValidationError::new("/activity_sha256", "invalid digest"));
        }
        if self.v != 1 {
            return Err(ValidationError::new("/v", "unsupported event ref version"));
        }
        require_string(&self.partition_key, "/partition_key", 1, 4_096)?;
        if self.expires_at <= 0 {
            return Err(ValidationError::new("/expires_at", "must be positive"));
        }
        validate_consumer(self.consumer.as_ref())?;
        if let Some(lineage) = &self.lineage {
            lineage.validate().map_err(|error| {
                ValidationError::new(format!("/lineage{}", error.pointer), error.message)
            })?;
        }
        match &self.reply {
            ReplyAddress::NativeLark(reply) => reply.validate()?,
            ReplyAddress::WhatsappGroup(reply) => reply.validate()?,
            ReplyAddress::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            } => {
                if [app_id, waba_id, phone_number_id, recipient_id]
                    .iter()
                    .any(|id| !crate::channel::whatsapp::valid_meta_id(id))
                    || !crate::channel::whatsapp::valid_message_id(message_id)
                {
                    return Err(ValidationError::new(
                        "/reply",
                        "invalid native WhatsApp destination",
                    ));
                }
            }
            ReplyAddress::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            } => {
                if thread_id.as_ref().is_some_and(|id| id != channel_id) {
                    return Err(ValidationError::new(
                        "/reply/thread_id",
                        "Discord thread must be the original channel",
                    ));
                }
                for (field, id) in [
                    ("bot_id", bot_id),
                    ("channel_id", channel_id),
                    ("message_id", message_id),
                ] {
                    if !crate::channel::discord::valid_id(id) {
                        return Err(ValidationError::new(
                            format!("/reply/{field}"),
                            "invalid Discord identity",
                        ));
                    }
                }
            }
            ReplyAddress::NyxidRelay {
                message_id,
                route_id,
                reply_token_expires_at,
                platform,
                thread_reply,
                ..
            } => {
                if *thread_reply && !matches!(platform, Platform::Lark | Platform::Feishu) {
                    return Err(ValidationError::new(
                        "/reply/thread_reply",
                        "native thread reply unavailable",
                    ));
                }
                require_string(message_id, "/reply/message_id", 1, 512)?;
                require_string(route_id, "/reply/route_id", 1, 512)?;
                if *reply_token_expires_at <= 0 {
                    return Err(ValidationError::new(
                        "/reply/reply_token_expires_at",
                        "must be positive",
                    ));
                }
            }
            ReplyAddress::Telegram {
                bot_id,
                chat_id,
                message_id,
                thread_id,
            } => {
                for (field, id, positive) in [
                    ("bot_id", bot_id.as_str(), true),
                    ("chat_id", chat_id.as_str(), false),
                    ("message_id", message_id.as_str(), true),
                ] {
                    if !id.parse::<i64>().is_ok_and(|n| {
                        (if positive { n > 0 } else { n != 0 }) && n.to_string() == id
                    }) {
                        return Err(ValidationError::new(
                            format!("/reply/{field}"),
                            "invalid Telegram identity",
                        ));
                    }
                }
                if thread_id.as_ref().is_some_and(|id| {
                    !id.parse::<i64>()
                        .is_ok_and(|n| n > 0 && n.to_string() == *id)
                }) {
                    return Err(ValidationError::new(
                        "/reply/thread_id",
                        "invalid Telegram thread",
                    ));
                }
            }
            ReplyAddress::ResultWebhook { url, hmac_key_id } => {
                validate_https_url(url, "/reply/url")?;
                require_string(hmac_key_id, "/reply/hmac_key_id", 1, 512)?;
            }
            ReplyAddress::Sync { waiter } => require_string(waiter, "/reply/waiter", 1, 512)?,
            ReplyAddress::None => {}
        }
        Ok(())
    }
}
