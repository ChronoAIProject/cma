//! Private wire codec preserves every existing sealed reply address.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
enum ReplyAddressWire {
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
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
            Self::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            } => ReplyAddressWire::Whatsapp {
                app_id: app_id.clone(),
                waba_id: waba_id.clone(),
                phone_number_id: phone_number_id.clone(),
                recipient_id: recipient_id.clone(),
                message_id: message_id.clone(),
            },
            Self::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            } => ReplyAddressWire::Discord {
                bot_id: bot_id.clone(),
                channel_id: channel_id.clone(),
                message_id: message_id.clone(),
                thread_id: thread_id.clone(),
            },
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
            ReplyAddressWire::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            } => Self::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            },
            ReplyAddressWire::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            } => Self::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            },
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
