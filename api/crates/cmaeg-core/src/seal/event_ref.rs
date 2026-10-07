//! Expiring event reply references sealed as `cmaeg1.ev` tokens.

use std::{fmt, str::FromStr};

use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;

use crate::{
    channel::{validate_https_url, Platform, Secret},
    ids::{ChannelId, ConversationId, EventId, RecordId},
    require_string,
    seal::{open, seal, MasterKeyRing, OpenError, Opened, SealError, TokenKind},
    ValidationError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventRefPayload {
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
}

#[derive(Clone, PartialEq, Eq, ToSchema)]
pub enum ReplyAddress {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
enum ReplyAddressWire {
    Telegram {
        bot_id: String,
        chat_id: String,
        message_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },

    NyxidRelay {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        thread_reply: bool,
        message_id: String,
        route_id: String,
        platform: Platform,
        reply_token: String,
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

impl Serialize for ReplyAddress {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match self {
            Self::NyxidRelay {
                message_id,
                route_id,
                platform,
                reply_token,
                reply_token_expires_at,
                thread_reply,
            } => ReplyAddressWire::NyxidRelay {
                thread_reply: *thread_reply,
                message_id: message_id.clone(),
                route_id: route_id.clone(),
                platform: *platform,
                reply_token: reply_token.as_str().to_owned(),
                reply_token_expires_at: *reply_token_expires_at,
            },
            Self::Telegram {
                bot_id,
                chat_id,
                message_id,
                thread_id,
            } => ReplyAddressWire::Telegram {
                bot_id: bot_id.clone(),
                chat_id: chat_id.clone(),
                message_id: message_id.clone(),
                thread_id: thread_id.clone(),
            },
            Self::ResultWebhook { url, hmac_key_id } => ReplyAddressWire::ResultWebhook {
                url: url.clone(),
                hmac_key_id: hmac_key_id.clone(),
            },
            Self::Sync { waiter } => ReplyAddressWire::Sync {
                waiter: waiter.clone(),
            },
            Self::None => ReplyAddressWire::None,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ReplyAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match ReplyAddressWire::deserialize(deserializer)? {
            ReplyAddressWire::NyxidRelay {
                message_id,
                route_id,
                platform,
                reply_token,
                reply_token_expires_at,
                thread_reply,
            } => Self::NyxidRelay {
                thread_reply,
                message_id,
                route_id,
                platform,
                reply_token: Secret::new(reply_token),
                reply_token_expires_at,
            },
            ReplyAddressWire::Telegram {
                bot_id,
                chat_id,
                message_id,
                thread_id,
            } => Self::Telegram {
                bot_id,
                chat_id,
                message_id,
                thread_id,
            },
            ReplyAddressWire::ResultWebhook { url, hmac_key_id } => {
                Self::ResultWebhook { url, hmac_key_id }
            }
            ReplyAddressWire::Sync { waiter } => Self::Sync { waiter },
            ReplyAddressWire::None => Self::None,
        })
    }
}

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
impl EventRefPayload {
    pub fn verifies_activity(&self, activity: &crate::activity::Activity) -> bool {
        self.activity_sha256
            .as_ref()
            .is_some_and(|digest| *digest == activity_digest(activity))
            && self.event_id == activity.event_id
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
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
        match &self.reply {
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
