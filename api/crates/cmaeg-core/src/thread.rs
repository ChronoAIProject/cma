//! Opt-in verified native-thread facts. These identifiers are not reply authority.
use crate::{
    activity::ActorKind,
    channel::{ChannelDefinition, Platform, Source},
    require_string, ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadContract {
    pub version: u16,
    /// Admit ordinary human messages in these chats, preserving sender/chat ACLs.
    #[serde(default)]
    pub follow_chat_ids: Vec<String>,
}
impl ThreadContract {
    pub fn validate(&self, channel: &ChannelDefinition) -> Result<(), ValidationError> {
        let pointer = "/reply/thread_contract";
        if self.version != 1 || !supported(channel) {
            return Err(ValidationError::new(
                pointer,
                "verified native thread contract unavailable",
            ));
        }
        if self.follow_chat_ids.len() > 256 {
            return Err(ValidationError::new(pointer, "at most 256 follow chats"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for id in &self.follow_chat_ids {
            require_string(id, pointer, 1, 512)?;
            if id.chars().any(char::is_control) || !seen.insert(id) {
                return Err(ValidationError::new(
                    pointer,
                    "follow chats must be unique identifiers",
                ));
            }
        }
        Ok(())
    }
}

pub fn supported(channel: &ChannelDefinition) -> bool {
    matches!(
        channel.sources.as_slice(),
        [Source::NyxidRelay {
            platform: Platform::Lark | Platform::Feishu,
            bot_id: Some(_),
            trust_normalized: false,
            ..
        }]
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadAddress {
    Mention,
    ReplyToBot,
    NotAddressed,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    Native,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadFacts {
    pub version: u16,
    pub kind: ThreadKind,
    pub chat_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
    pub sender_kind: ActorKind,
    pub address: ThreadAddress,
    #[serde(default)]
    pub mentions_others: bool,
}
impl std::fmt::Debug for ThreadFacts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadFacts")
            .field("version", &self.version)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
impl ThreadFacts {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.version != 1 || self.sender_kind != ActorKind::Human {
            return Err(ValidationError::new("/thread", "unsupported thread facts"));
        }
        for id in [
            Some(&self.chat_id),
            Some(&self.message_id),
            self.root_id.as_ref(),
            self.native_thread_id.as_ref(),
            self.parent_message_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            require_string(id, "/thread", 1, 512)?;
            if id.chars().any(char::is_control) {
                return Err(ValidationError::new("/thread", "invalid identifier"));
            }
        }
        Ok(())
    }
}
