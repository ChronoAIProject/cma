use serde::{Deserialize, Serialize};

use crate::bridge::ingress::StoredInbox;
use crate::bridge::BridgePlatform;

use super::{opaque_id, PartitionError, PartitionSourcePin};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelConversationKind {
    Private,
    Group,
    Channel,
    Unknown,
}

impl ChannelConversationKind {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
            Self::Channel => "channel",
            Self::Unknown => "unknown",
        }
    }

    pub(super) fn from_wire(value: &str) -> Self {
        match value {
            "private" => Self::Private,
            "group" => Self::Group,
            "channel" => Self::Channel,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSenderKind {
    Human,
    Bot,
    System,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelEventKind {
    UserMessage,
    AttachmentMessage,
    UnsupportedContent,
    MessageEdit,
    InteractiveAction,
    SystemEvent,
    CallbackEcho,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopEvidence {
    NotLoop,
    Loop,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedFlag {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StableThreadKind {
    TelegramForumTopic,
    DiscordChannelThread,
    LarkThread,
    FeishuThread,
    SlackThread,
}

/// The connector must obtain this from a documented stable platform field.
/// No Deserialize/Debug: a generic callback thread value is not this evidence.
#[derive(Clone, PartialEq, Eq)]
pub struct StableThreadReference {
    kind: StableThreadKind,
    id: String,
}

impl StableThreadReference {
    pub fn from_verified_field(kind: StableThreadKind, id: String) -> Result<Self, PartitionError> {
        // Explicitly reject NyxID's Discord interaction routing representation,
        // even if an adapter mistakenly forwards it as a bounded stable field.
        if !opaque_id(&id) || id.starts_with("interaction:") {
            return Err(PartitionError::InvalidSource("stable_thread"));
        }
        Ok(Self { kind, id })
    }
    pub fn kind(&self) -> StableThreadKind {
        self.kind
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn require_platform(&self, platform: &BridgePlatform) -> Result<(), PartitionError> {
        if !matches!(
            (self.kind, platform),
            (
                StableThreadKind::TelegramForumTopic,
                BridgePlatform::Telegram | BridgePlatform::TelegramNew
            ) | (
                StableThreadKind::DiscordChannelThread,
                BridgePlatform::Discord
            ) | (StableThreadKind::LarkThread, BridgePlatform::Lark)
                | (StableThreadKind::FeishuThread, BridgePlatform::Feishu)
                | (StableThreadKind::SlackThread, BridgePlatform::Slack)
        ) {
            return Err(PartitionError::InvalidSource("stable_thread_platform"));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum ThreadEvidence {
    Unknown,
    Unsupported,
    Missing,
    Verified(StableThreadReference),
}

/// Trusted adapter output, deliberately without Deserialize or Debug. The
/// exact source pin and identities prevent applying another message's facts.
/// Missing provider metadata MUST remain Unknown, including absent bot flags.
#[derive(Clone)]
pub struct PlatformCapabilityObservation {
    pub source: PartitionSourcePin,
    pub platform: BridgePlatform,
    pub chat_id: String,
    pub sender_id: String,
    pub provider_message_id: Option<String>,
    pub conversation: ChannelConversationKind,
    pub sender_kind: ChannelSenderKind,
    pub event_kind: ChannelEventKind,
    pub loop_evidence: LoopEvidence,
    pub mentioned_bot: VerifiedFlag,
    pub replied_to_bot: VerifiedFlag,
    /// Exact permanent CMA Sent item used to prove selected-bot reply addressing.
    /// Historical evidence only; never authorizes another send or execution.
    pub reply_receipt: Option<ConfirmedReplyReceipt>,
    pub thread: ThreadEvidence,
}

impl PlatformCapabilityObservation {
    /// A generic signed inbox proves source correlation, but cannot establish
    /// human/non-loop/group-addressing/stable-thread capabilities by absence.
    pub fn unknown(inbox: &StoredInbox) -> Result<Self, PartitionError> {
        Ok(Self {
            source: PartitionSourcePin::for_inbox(inbox)?,
            platform: super::platform(&inbox.envelope.platform)?,
            chat_id: inbox.envelope.chat_id.clone(),
            sender_id: inbox.envelope.sender_id.clone(),
            provider_message_id: None,
            conversation: ChannelConversationKind::from_wire(&inbox.envelope.conversation_type),
            sender_kind: ChannelSenderKind::Unknown,
            event_kind: ChannelEventKind::Unknown,
            loop_evidence: LoopEvidence::Unknown,
            mentioned_bot: VerifiedFlag::Unknown,
            replied_to_bot: VerifiedFlag::Unknown,
            reply_receipt: None,
            thread: ThreadEvidence::Unknown,
        })
    }
}

/// Exact immutable evidence checked against a permanent Sent outbox record in
/// the callback acceptance transaction. This is not permission to send again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedReplyReceipt {
    pub item_id: String,
    pub revision: u64,
    pub record_sha256: String,
}
impl ConfirmedReplyReceipt {
    pub fn validate(&self) -> Result<(), PartitionError> {
        if !self
            .item_id
            .strip_prefix("bro_")
            .is_some_and(crate::bridge::partition::digest)
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || !crate::bridge::partition::digest(&self.record_sha256)
        {
            return Err(PartitionError::InvalidSource("reply_addressing_evidence"));
        }
        Ok(())
    }
}
