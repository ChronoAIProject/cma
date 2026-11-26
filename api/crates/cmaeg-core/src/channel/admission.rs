use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum AdmissionPolicy {
    All {},
    Allowlist {
        sender_ids: Vec<String>,
    },
    Scoped {
        senders: IdentityAdmission,
        chats: IdentityAdmission,
        groups: GroupAdmission,
        /// Native exact bot mentions only. The caller must bound downstream
        /// automated execution; the gateway grants no human execution identity.
        #[serde(default, skip_serializing_if = "BotMessageAdmission::is_deny")]
        bot_messages: BotMessageAdmission,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BotMessageAdmission {
    #[default]
    Deny,
    NativeMentions,
}
impl BotMessageAdmission {
    pub fn is_deny(&self) -> bool {
        *self == Self::Deny
    }
}
impl AdmissionPolicy {
    pub fn native_bot_mentions(&self) -> bool {
        matches!(
            self,
            Self::Scoped {
                bot_messages: BotMessageAdmission::NativeMentions,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(
    rename_all = "snake_case",
    tag = "type",
    content = "ids",
    deny_unknown_fields
)]
pub enum IdentityAdmission {
    Deny,
    Open,
    Allowlist(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupAdmission {
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}
