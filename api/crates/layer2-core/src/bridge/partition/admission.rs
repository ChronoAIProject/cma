use serde::{Deserialize, Serialize};

use crate::bridge::{BridgeAdmissionPolicy, BridgeConfig, BridgePartitionPolicy};

use super::{
    ChannelConversationKind, ChannelEventKind, ChannelSenderKind, GroupAdmission, LoopEvidence,
    NormalizedPartitionKey, NormalizedPartitionSource, PartitionContextSharing, PartitionError,
    ReplyVisibility, ThreadEvidence, VerifiedFlag,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionRefusal {
    ConfigurationUnsupported,
    UpstreamDenied,
    UnknownSender,
    BotSender,
    SystemSender,
    UnknownEvent,
    AttachmentDownloadRequired,
    ContentUnsupported,
    MessageEdit,
    InteractiveActionUnsupported,
    SystemEvent,
    CallbackLoop,
    UnknownLoopEvidence,
    UnsupportedConversation,
    SenderDenied,
    ChatDenied,
    GroupDenied,
    AddressingEvidenceUnavailable,
    GroupNotAddressed,
    ThreadEvidenceUnavailable,
    ThreadUnsupported,
    ThreadMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionAdmissionDecision {
    Admitted {
        key: NormalizedPartitionKey,
        context: PartitionContextSharing,
        reply_visibility: ReplyVisibility,
    },
    Refused {
        reason: PartitionRefusal,
    },
}

impl NormalizedPartitionSource {
    /// Pure classification under this source's exact pinned configuration.
    /// Admitted grants no allocation, invoke, queue, reset or reply permission.
    pub fn admit(
        &self,
        config: &BridgeConfig,
    ) -> Result<PartitionAdmissionDecision, PartitionError> {
        self.require_config(config)?;
        if let Some(reason) = self.refusal(config) {
            return Ok(PartitionAdmissionDecision::Refused { reason });
        }
        let thread = if matches!(
            config.partition,
            BridgePartitionPolicy::Thread | BridgePartitionPolicy::ThreadSender
        ) {
            match &self.observation.thread {
                ThreadEvidence::Verified(reference) => Some(reference.id()),
                _ => return Err(PartitionError::InvalidSource("stable_thread")),
            }
        } else {
            None
        };
        Ok(PartitionAdmissionDecision::Admitted {
            key: NormalizedPartitionKey::for_source(self, config.partition, thread)?,
            context: config.partition.into(),
            reply_visibility: if self.conversation_kind() == ChannelConversationKind::Private {
                ReplyVisibility::OriginalPrivateChat
            } else {
                ReplyVisibility::OriginalGroupOrChannel
            },
        })
    }

    fn refusal(&self, config: &BridgeConfig) -> Option<PartitionRefusal> {
        use PartitionRefusal as R;
        if !self.upstream_admitted {
            return Some(R::UpstreamDenied);
        }
        let facts = &self.observation;
        if facts.event_kind == ChannelEventKind::InteractiveAction {
            return Some(R::InteractiveActionUnsupported);
        }
        if facts.event_kind == ChannelEventKind::MessageEdit {
            return Some(R::MessageEdit);
        }
        match facts.sender_kind {
            ChannelSenderKind::Unknown => return Some(R::UnknownSender),
            ChannelSenderKind::Bot => return Some(R::BotSender),
            ChannelSenderKind::System => return Some(R::SystemSender),
            ChannelSenderKind::Human => {}
        }
        match facts.event_kind {
            ChannelEventKind::MessageEdit => return Some(R::MessageEdit),
            ChannelEventKind::InteractiveAction => return Some(R::InteractiveActionUnsupported),
            ChannelEventKind::Unknown => return Some(R::UnknownEvent),
            ChannelEventKind::AttachmentMessage => return Some(R::AttachmentDownloadRequired),
            ChannelEventKind::UnsupportedContent => return Some(R::ContentUnsupported),
            ChannelEventKind::SystemEvent => return Some(R::SystemEvent),
            ChannelEventKind::CallbackEcho => return Some(R::CallbackLoop),
            ChannelEventKind::UserMessage | ChannelEventKind::DiscordTextAttachment => {}
        }
        match facts.loop_evidence {
            LoopEvidence::Unknown => return Some(R::UnknownLoopEvidence),
            LoopEvidence::Loop => return Some(R::CallbackLoop),
            LoopEvidence::NotLoop => {}
        }
        if facts.conversation == ChannelConversationKind::Unknown {
            return Some(R::UnsupportedConversation);
        }
        let group = match &config.admission {
            BridgeAdmissionPolicy::All => GroupAdmission::All,
            BridgeAdmissionPolicy::Allowlist { sender_ids } => {
                if !sender_ids.contains(&facts.sender_id) {
                    return Some(R::SenderDenied);
                }
                GroupAdmission::All
            }
            BridgeAdmissionPolicy::Scoped { policy } => {
                if !policy.senders.permits(&facts.sender_id) {
                    return Some(R::SenderDenied);
                }
                if !policy.chats.permits(&facts.chat_id) {
                    return Some(R::ChatDenied);
                }
                policy.groups
            }
        };
        if facts.conversation != ChannelConversationKind::Private {
            if let Some(reason) = group_refusal(group, facts.mentioned_bot, facts.replied_to_bot) {
                return Some(reason);
            }
        }
        if matches!(
            config.partition,
            BridgePartitionPolicy::Thread | BridgePartitionPolicy::ThreadSender
        ) {
            match facts.thread {
                ThreadEvidence::Unknown => return Some(R::ThreadEvidenceUnavailable),
                ThreadEvidence::Unsupported => return Some(R::ThreadUnsupported),
                ThreadEvidence::Missing => return Some(R::ThreadMissing),
                ThreadEvidence::Verified(_) => {}
            }
        }
        None
    }
}

fn group_refusal(
    policy: GroupAdmission,
    mentioned: VerifiedFlag,
    replied: VerifiedFlag,
) -> Option<PartitionRefusal> {
    use PartitionRefusal as R;
    use VerifiedFlag as F;
    let addressed = match policy {
        GroupAdmission::Deny => return Some(R::GroupDenied),
        GroupAdmission::All => return None,
        GroupAdmission::Mention => mentioned,
        GroupAdmission::ReplyToBot => replied,
        GroupAdmission::MentionOrReplyToBot => match (mentioned, replied) {
            (F::Yes, _) | (_, F::Yes) => F::Yes,
            (F::No, F::No) => F::No,
            _ => F::Unknown,
        },
    };
    match addressed {
        F::Yes => None,
        F::No => Some(R::GroupNotAddressed),
        F::Unknown => Some(R::AddressingEvidenceUnavailable),
    }
}
