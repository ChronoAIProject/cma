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

pub const ACTIVITY_SCHEMA_VERSION: u16 = 1;
pub const MAX_ATTACHMENTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Activity {
    pub schema_version: u16,
    pub event_id: EventId,
    pub kind: ActivityKind,
    pub actor: Actor,
    pub conversation: ActivityConversation,
    pub occurred_at: String,
    pub source: SourceKind,
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
    },
    Interaction(InteractionResult),
    Command {
        name: String,
        args: String,
    },
    Event {
        ce: CloudEvent,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub reference: String,
    pub kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    File,
    Audio,
    Video,
    Other,
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
    Telegram {
        platform: Platform,
        bot_id: String,
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
        if self.schema_version != ACTIVITY_SCHEMA_VERSION {
            return Err(ValidationError::new(
                "/schema_version",
                "unsupported schema version",
            ));
        }
        require_string(self.event_id.as_str(), "/event_id", 1, 512)?;
        let identity_limit = if matches!(self.source, SourceKind::Custom) {
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
                text, attachments, ..
            } => {
                require_string(text, "/kind/text", 0, 32 * 1_024)?;
                if attachments.len() > MAX_ATTACHMENTS {
                    return Err(ValidationError::new(
                        "/kind/attachments",
                        "too many attachments",
                    ));
                }
                for (index, attachment) in attachments.iter().enumerate() {
                    require_string(
                        &attachment.reference,
                        &format!("/kind/attachments/{index}/reference"),
                        1,
                        4_096,
                    )?;
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
            ActivityKind::Event { ce } => ce.validate().map_err(|error| {
                ValidationError::new(format!("/kind/ce{}", error.pointer), error.message)
            })?,
        }
        Ok(())
    }

    /// Compute the stable serialized partition key selected by a channel.
    pub fn partition_key(&self, rule: &PartitionRule) -> String {
        match rule.key {
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
                ActivityKind::Event { ce } => {
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
