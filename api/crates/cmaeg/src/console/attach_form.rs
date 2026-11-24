//! The console's attach forms as management requests: one form per source
//! type, mirroring `SourceRequest`, parsed into an [`AttachRequest`].
//!
//! Parsing is structural only; management applies the same validation as
//! REST. What an owner typed is kept for a re-render, except every secret
//! field, so an error page never carries a submitted token, key or secret.
use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::channel::{
    AdmissionPolicy, CustomAttachments, CustomAuth, GroupAdmission, IdentityAdmission,
    PartitionKey, Platform, Secret,
};
use ring::rand::{SecureRandom, SystemRandom};

use super::source_changes::{AttachCredentials, AttachRequest};
use crate::channels::{HmacSecretRequest, SourceRequest};

/// Fields that carry a secret: never kept, never re-rendered.
pub(crate) const SECRET_FIELDS: &[&str] = &[
    "telegram_bot_token",
    "telegram_webhook_secret",
    "channel_key",
    "hmac_secret",
];
/// Bytes of a generated HMAC secret.
const GENERATED_BYTES: usize = 32;
const PLATFORMS: [Platform; 6] = [
    Platform::Telegram,
    Platform::Lark,
    Platform::Feishu,
    Platform::Discord,
    Platform::Slack,
    Platform::Whatsapp,
];

/// The non-secret fields of a submitted form, to re-render it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Submitted {
    values: HashMap<String, String>,
}

impl Submitted {
    pub(crate) fn of(form: &[u8]) -> Self {
        let values = url::form_urlencoded::parse(form)
            .filter(|(name, _)| name != "csrf" && !SECRET_FIELDS.contains(&name.as_ref()))
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        Self { values }
    }

    /// The form's source type: `telegram`, `nyxid_relay` or `custom`.
    pub(crate) fn kind(&self) -> &str {
        self.value("type")
    }

    pub(crate) fn value(&self, name: &str) -> &str {
        self.values.get(name).map_or("", String::as_str)
    }
}

/// A secret the gateway generated for this attach, shown once.
pub(crate) struct Generated {
    pub key_id: String,
    pub secret: Secret,
}

pub(crate) struct Parsed {
    pub request: AttachRequest,
    pub generated: Option<Generated>,
}

/// Parse a submitted attach form. The error is fixed console text.
pub(crate) fn parse(form: &[u8]) -> Result<Parsed, &'static str> {
    let fields = Fields::of(form)?;
    let expected_version = fields
        .text("expected_version")
        .parse()
        .map_err(|_| "This form is out of date. Reload the page and try again.")?;
    let mut credentials = AttachCredentials::default();
    let mut generated = None;
    let mut partition_custom_key = None;
    let source = match fields.text("type") {
        "telegram" => {
            credentials.telegram_bot_token = fields.secret("telegram_bot_token");
            credentials.telegram_webhook_secret = fields.secret("telegram_webhook_secret");
            SourceRequest::Telegram {
                topic_lifecycle: false,
                topic_creation: false,
                message_events: false,
                channel_posts: false,
                member_join_chat_ids: None,
                bot_id: fields.required("bot_id", "Enter the bot id.")?,
                bot_username: fields.optional("bot_username"),
                admission: admission(&fields)?,
            }
        }
        "nyxid_relay" => {
            credentials.channel_key = fields.secret("channel_key");
            let platform = fields.text("platform");
            SourceRequest::NyxidRelay {
                issuer: fields.required("issuer", "Enter the relay issuer.")?,
                key_id: fields.required("key_id", "Enter the relay key id.")?,
                route_ids: fields.list("route_ids"),
                platform: PLATFORMS
                    .into_iter()
                    .find(|known| known.as_str() == platform)
                    .ok_or("Choose a platform.")?,
                bot_username: fields.optional("bot_username"),
                bot_id: fields.optional("bot_id"),
                trust_normalized: fields.checked("trust_normalized"),
                verify_raw_event: fields.checked("verify_raw_event"),
                lark_message_effects: None,
                lark_resources: None,
                member_join_chat_ids: None,
                admission: admission(&fields)?,
            }
        }
        "custom" => {
            let auth = match fields.text("auth") {
                "hmac" => {
                    let key_id = fields.required("key_id", "Enter the HMAC key id.")?;
                    let secret = if fields.checked("generate_secret") {
                        let secret = generate()?;
                        generated = Some(Generated {
                            key_id: key_id.clone(),
                            secret: secret.clone(),
                        });
                        Some(secret)
                    } else {
                        fields.secret("hmac_secret")
                    };
                    if let Some(secret) = secret {
                        credentials.hmac_secrets.push(HmacSecretRequest {
                            key_id: key_id.clone(),
                            secret,
                        });
                    }
                    CustomAuth::Hmac { key_id }
                }
                "nyxid_key" => CustomAuth::NyxidKey {
                    key_hashes: fields.list("key_hashes"),
                },
                _ => return Err("Choose how the source authenticates."),
            };
            partition_custom_key = match fields.text("partition_custom_key") {
                "" => None,
                value => Some(partition_key(value).ok_or("Choose a partition key.")?),
            };
            let schema = match fields.optional("schema") {
                Some(text) => Some(
                    serde_json::from_str(&text).map_err(|_| "The event schema must be JSON.")?,
                ),
                None => None,
            };
            SourceRequest::Custom {
                auth,
                prompt_template: fields.optional("prompt_template"),
                schema,
                attachments: fields.optional("attachments_pointer").map(|pointer| {
                    CustomAttachments {
                        pointer,
                        origins: fields.list("attachments_origins"),
                    }
                }),
            }
        }
        _ => return Err("Choose a source type."),
    };
    Ok(Parsed {
        request: AttachRequest {
            expected_version,
            source,
            credentials,
            partition_custom_key,
        },
        generated,
    })
}

/// The admission fields shared by Telegram and relay forms.
fn admission(fields: &Fields) -> Result<AdmissionPolicy, &'static str> {
    match fields.text("admission") {
        "" | "all" => Ok(AdmissionPolicy::All {}),
        "allowlist" => Ok(AdmissionPolicy::Allowlist {
            sender_ids: fields.list("sender_ids"),
        }),
        "scoped" => Ok(AdmissionPolicy::Scoped {
            bot_messages: Default::default(),
            senders: identity(fields.text("senders"), fields.list("sender_ids"))?,
            chats: identity(fields.text("chats"), fields.list("chat_ids"))?,
            groups: match fields.text("groups") {
                "deny" => GroupAdmission::Deny,
                "all" => GroupAdmission::All,
                "mention" => GroupAdmission::Mention,
                "reply_to_bot" => GroupAdmission::ReplyToBot,
                "mention_or_reply_to_bot" => GroupAdmission::MentionOrReplyToBot,
                _ => return Err("Choose how groups are admitted."),
            },
        }),
        _ => Err("Choose an admission policy."),
    }
}

fn identity(mode: &str, ids: Vec<String>) -> Result<IdentityAdmission, &'static str> {
    match mode {
        "open" => Ok(IdentityAdmission::Open),
        "deny" => Ok(IdentityAdmission::Deny),
        "allowlist" => Ok(IdentityAdmission::Allowlist(ids)),
        _ => Err("Choose how senders and chats are admitted."),
    }
}

fn partition_key(value: &str) -> Option<PartitionKey> {
    Some(match value {
        "conversation_and_sender" => PartitionKey::ConversationAndSender,
        "conversation" => PartitionKey::Conversation,
        "thread" => PartitionKey::Thread,
        "fixed" => PartitionKey::Fixed,
        "event_subject" => PartitionKey::EventSubject,
        "per_event" => PartitionKey::PerEvent,
        _ => return None,
    })
}

/// 32 random bytes, base64.
fn generate() -> Result<Secret, &'static str> {
    let mut bytes = [0u8; GENERATED_BYTES];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        tracing::error!("console could not generate an HMAC secret");
        "A secret could not be generated. Try again."
    })?;
    Ok(Secret::new(STANDARD.encode(bytes)))
}

/// A form's fields, each given at most once.
struct Fields(HashMap<String, String>);

impl Fields {
    fn of(form: &[u8]) -> Result<Self, &'static str> {
        let mut fields = HashMap::new();
        for (name, value) in url::form_urlencoded::parse(form) {
            if fields
                .insert(name.into_owned(), value.into_owned())
                .is_some()
            {
                return Err("This form could not be read. Reload the page and try again.");
            }
        }
        Ok(Self(fields))
    }

    fn text(&self, name: &str) -> &str {
        self.0.get(name).map_or("", |value| value.trim())
    }

    fn optional(&self, name: &str) -> Option<String> {
        Some(self.text(name).to_owned()).filter(|value| !value.is_empty())
    }

    fn required(&self, name: &str, missing: &'static str) -> Result<String, &'static str> {
        self.optional(name).ok_or(missing)
    }

    /// A secret exactly as typed; empty is absent.
    fn secret(&self, name: &str) -> Option<Secret> {
        self.0
            .get(name)
            .filter(|value| !value.is_empty())
            .map(|value| Secret::new(value.as_str()))
    }

    fn checked(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// Items separated by commas or line breaks.
    fn list(&self, name: &str) -> Vec<String> {
        self.text(name)
            .split([',', '\n'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(test)]
#[path = "attach_form_tests.rs"]
mod tests;
