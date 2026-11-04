//! Neutral activity emitted by an event source.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Map;
use utoipa::ToSchema;

use crate::{
    channel::{PartitionKey, PartitionRule, Platform},
    cloudevent::CloudEvent,
    ids::{EventId, MessageRef},
    require_string, ValidationError,
};

mod quoted_message;
pub use quoted_message::{
    QuoteRelation, QuoteTextOrigin, QuotedContent, QuotedContentKind, QuotedMessage, QuotedText,
    MAX_QUOTED_TEXT_BYTES,
};

mod attachment;
pub use attachment::{
    forbidden_text_char, https_locator, mime_restricted_name, telegram_ids_valid,
    validate_attachments, Attachment, AttachmentKind, AttachmentSource, MediaVariant,
    MAX_ALBUM_ID_BYTES, MAX_ATTACHMENTS, MAX_ATTACHMENT_URL_BYTES, MAX_DIMENSION,
    MAX_DURATION_SECONDS, MAX_EMOJI_BYTES, MAX_FILENAME_BYTES, MAX_FILE_ID_BYTES,
    MAX_FILE_UNIQUE_ID_BYTES, MAX_INLINE_POINTER_BYTES, MAX_MIME_PART_BYTES, MAX_NYXID_INDEX,
    MAX_REFERENCE_BYTES,
};
mod attachment_facts;
pub use attachment_facts::FactRepairs;
mod routing;
pub(crate) use routing::is_root_id;
pub use routing::{ActivityRouting, Causation, ConsumerTarget, RouteResolution, MAX_HOPS};

pub const ACTIVITY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Activity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<crate::thread::ThreadFacts>,
    pub schema_version: u16,
    pub event_id: EventId,
    pub kind: ActivityKind,
    pub actor: Actor,
    pub conversation: ActivityConversation,
    pub occurred_at: String,
    pub source: SourceKind,
    /// The consumers a routed event reaches; absent on every unrouted event,
    /// so an activity without routing keeps its exact bytes and digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<ActivityRouting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum ActivityKind {
    Message {
        text: String,
        #[serde(default)]
        attachments: Vec<Attachment>,
        mentions_bot: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<MessageRef>,
        /// Untrusted native reply context, scoped by this activity; grants no authority.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quoted_message: Option<QuotedMessage>,
        /// Shared by every message of one platform media group (a Telegram
        /// album). Each item of the group is still its own activity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        album_id: Option<String>,
    },
    Interaction(InteractionResult),
    Command {
        name: String,
        args: String,
    },
    Event {
        ce: CloudEvent,
        /// Media the custom source extracted from the event; omitted when empty
        /// so events stored without attachments keep their exact bytes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub kind: ActorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Bot,
    System,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivityConversation {
    pub id: String,
    pub kind: ConversationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Private,
    Group,
    Channel,
    Thread,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum SourceKind {
    NativeLark(crate::channel::NativeLarkIdentity),
    Whatsapp {
        app_id: String,
        waba_id: String,
        phone_number_id: String,
    },
    Telegram {
        platform: Platform,
        bot_id: String,
    },
    Discord {
        bot_id: String,
    },
    Github {
        app_ref: String,
        installation_id: u64,
        repository_id: u64,
    },
    NyxidRelay {
        platform: Platform,
        route_id: String,
    },
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InteractionResult {
    pub block_id: String,
    pub action_id: String,
    #[serde(default)]
    pub values: Map<String, serde_json::Value>,
}

impl Activity {
    /// Validate the bounded neutral event representation.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let SourceKind::NativeLark(identity) = &self.source {
            identity.validate()?;
        }
        if let SourceKind::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
        } = &self.source
        {
            if [app_id, waba_id, phone_number_id]
                .iter()
                .any(|id| !crate::channel::whatsapp::valid_meta_id(id))
            {
                return Err(ValidationError::new(
                    "/source",
                    "native WhatsApp requires numeric Meta identities",
                ));
            }
        }
        if let SourceKind::Telegram { platform, bot_id } = &self.source {
            if *platform != Platform::Telegram
                || !bot_id
                    .parse::<i64>()
                    .is_ok_and(|id| id > 0 && id.to_string() == *bot_id)
            {
                return Err(ValidationError::new(
                    "/source",
                    "native Telegram requires its positive bot ID and telegram platform",
                ));
            }
        }
        if let SourceKind::Discord { bot_id } = &self.source {
            if !crate::channel::discord::valid_id(bot_id) {
                return Err(ValidationError::new(
                    "/source/bot_id",
                    "must be a canonical Discord ID",
                ));
            }
        }
        if self.schema_version != ACTIVITY_SCHEMA_VERSION {
            return Err(ValidationError::new(
                "/schema_version",
                "unsupported schema version",
            ));
        }
        if let Some(thread) = &self.thread {
            thread.validate()?;
            if thread.chat_id != self.conversation.id
                || thread.sender_kind != self.actor.kind
                || self.conversation.kind == ConversationKind::Private
            {
                return Err(ValidationError::new("/thread", "thread identity mismatch"));
            }
        }
        require_string(self.event_id.as_str(), "/event_id", 1, 512)?;
        let identity_limit =
            if matches!(self.source, SourceKind::Custom | SourceKind::Github { .. }) {
                4096
            } else {
                512
            };
        require_string(&self.actor.id, "/actor/id", 1, identity_limit)?;
        require_string(&self.conversation.id, "/conversation/id", 1, identity_limit)?;
        require_string(&self.occurred_at, "/occurred_at", 1, 128)?;
        if let Some(thread_id) = &self.conversation.thread_id {
            require_string(thread_id, "/conversation/thread_id", 1, 512)?;
        }
        match &self.kind {
            ActivityKind::Message {
                text,
                attachments,
                album_id,
                quoted_message,
                ..
            } => {
                require_string(text, "/kind/text", 0, 32 * 1_024)?;
                if let Some(quote) = quoted_message {
                    quote.validate(&self.source)?;
                }
                validate_attachments(attachments, &self.source, "/kind/attachments")?;
                if let Some(album_id) = album_id {
                    attachment::require_text(album_id, "/kind/album_id", MAX_ALBUM_ID_BYTES)?;
                }
            }
            ActivityKind::Interaction(result) => {
                require_string(&result.block_id, "/kind/block_id", 1, 512)?;
                require_string(&result.action_id, "/kind/action_id", 1, 512)?;
            }
            ActivityKind::Command { name, args } => {
                require_string(name, "/kind/name", 1, 512)?;
                require_string(args, "/kind/args", 0, 32 * 1_024)?;
            }
            ActivityKind::Event { ce, attachments } => {
                ce.validate().map_err(|error| {
                    ValidationError::new(format!("/kind/ce{}", error.pointer), error.message)
                })?;
                validate_attachments(attachments, &self.source, "/kind/attachments")?;
            }
        }
        if let Some(routing) = &self.routing {
            routing.validate(&self.kind)?;
        }
        Ok(())
    }

    /// Compute the stable serialized partition key selected by a channel for
    /// this activity's source: `custom_key` for a custom event when the rule
    /// carries one, and `key` otherwise.
    pub fn partition_key(&self, rule: &PartitionRule) -> String {
        match rule.key_for(&self.source) {
            PartitionKey::ConversationAndSender => {
                format!("{}\u{1f}{}", self.conversation.id, self.actor.id)
            }
            PartitionKey::Conversation => self.conversation.id.clone(),
            PartitionKey::Thread => format!(
                "{}\u{1f}{}",
                self.conversation.id,
                self.conversation.thread_id.as_deref().unwrap_or_default()
            ),
            PartitionKey::Fixed => "fixed".to_owned(),
            PartitionKey::EventSubject => match &self.kind {
                ActivityKind::Event { ce, .. } => {
                    ce.subject.clone().unwrap_or_else(|| ce.source.clone())
                }
                _ => self.conversation.id.clone(),
            },
            PartitionKey::PerEvent => self.event_id.to_string(),
        }
    }
}

/// Keep these imports available to protocol users without making them depend
/// on an implementation module for common map construction.
pub type ActivityExtensions = BTreeMap<String, serde_json::Value>;
