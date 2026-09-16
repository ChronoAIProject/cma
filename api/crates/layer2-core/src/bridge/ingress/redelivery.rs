//! Request-local provider message equivalence from an authenticated original event.
use super::{AuthenticatedCallbackPayload, CallbackError, StoredInbox};
use crate::bridge::partition::{
    ChannelEventKind, ChannelSenderKind, LoopEvidence, PartitionSourcePin,
    PlatformCapabilityObservation, StableThreadKind, ThreadEvidence,
};
use serde::Serialize;

/// The trusted platform boundary has corroborated this against raw message fields.
/// No Deserialize: a generic reply field or an interaction token cannot create it.
pub enum VerifiedRedeliveryParent {
    Absent,
    Message(String),
}

/// Binds a neutral semantic projection to one exact authenticated callback.
/// This descriptor carries no admission, execution, download or reply authority.
pub struct VerifiedPlatformRedelivery {
    source: PartitionSourcePin,
    key_sha256: String,
    semantic_sha256: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Thread<'a> {
    Unknown,
    Unsupported,
    Missing,
    Verified {
        platform_kind: StableThreadKind,
        id: &'a str,
    },
}

impl VerifiedPlatformRedelivery {
    /// Only a trusted adapter with an authenticated body may report these facts.
    pub fn from_verified_observation(
        payload: &AuthenticatedCallbackPayload<'_>,
        facts: &PlatformCapabilityObservation,
        parent: VerifiedRedeliveryParent,
    ) -> Result<Option<Self>, CallbackError> {
        Self::with_attachment(payload, facts, parent, None)
    }

    pub fn from_verified_text_attachment(
        payload: &AuthenticatedCallbackPayload<'_>,
        facts: &PlatformCapabilityObservation,
        parent: VerifiedRedeliveryParent,
        attachment: &super::VerifiedTextAttachment,
    ) -> Result<Option<Self>, CallbackError> {
        attachment.metadata().validate(payload.record())?;
        Self::with_attachment(payload, facts, parent, Some(attachment.metadata()))
    }

    fn with_attachment(
        payload: &AuthenticatedCallbackPayload<'_>,
        facts: &PlatformCapabilityObservation,
        parent: VerifiedRedeliveryParent,
        attachment: Option<&super::DiscordTextAttachment>,
    ) -> Result<Option<Self>, CallbackError> {
        let inbox = payload.record();
        let kind_matches = match attachment {
            Some(_) => facts.event_kind == ChannelEventKind::DiscordTextAttachment,
            None => facts.event_kind == ChannelEventKind::UserMessage,
        };
        if !kind_matches
            || facts.sender_kind != ChannelSenderKind::Human
            || facts.loop_evidence != LoopEvidence::NotLoop
            || inbox.envelope.content_type != "text"
            || !inbox.envelope.attachments.is_empty()
            || inbox.envelope.text.is_none()
        {
            return Ok(None);
        }
        let Some(message) = facts.provider_message_id.as_deref() else {
            return Ok(None);
        };
        let source = PartitionSourcePin::for_inbox(inbox).map_err(|_| CallbackError::Payload)?;
        let platform = serde_json::to_value(&facts.platform).map_err(|_| CallbackError::Payload)?;
        if source != facts.source
            || facts.chat_id != inbox.envelope.chat_id
            || facts.sender_id != inbox.envelope.sender_id
            || platform.as_str() != Some(inbox.envelope.platform.as_str())
            || message.is_empty()
            || message.len() > 256
            || message.chars().any(char::is_control)
        {
            return Err(CallbackError::Payload);
        }
        let parent = match &parent {
            VerifiedRedeliveryParent::Absent => None,
            VerifiedRedeliveryParent::Message(id) => Some(id.as_str()),
        };
        if parent != inbox.envelope.reply_to_platform_message_id.as_deref()
            || parent.is_some_and(|id| {
                id.is_empty() || id.len() > 256 || id.chars().any(char::is_control)
            })
        {
            return Err(CallbackError::Payload);
        }
        let thread = match &facts.thread {
            ThreadEvidence::Unknown => Thread::Unknown,
            ThreadEvidence::Unsupported => Thread::Unsupported,
            ThreadEvidence::Missing => Thread::Missing,
            ThreadEvidence::Verified(value) => Thread::Verified {
                platform_kind: value.kind(),
                id: value.id(),
            },
        };
        let digest = |value: &serde_json::Value| {
            crate::profiles::content_digest(value).map_err(|_| CallbackError::Payload)
        };
        let key_sha256 = digest(&serde_json::json!([
            "cma.bridge.platform-message.v1",
            inbox.owner,
            inbox.pin.bridge_id,
            inbox.attestation.issuer,
            inbox.bot_id,
            facts.platform,
            facts.chat_id,
            message
        ]))?;
        let text_semantics = serde_json::json!([
            "cma.bridge.platform-text.v1",
            facts.sender_id,
            facts.event_kind,
            facts.sender_kind,
            facts.loop_evidence,
            facts.conversation,
            thread,
            parent,
            inbox.envelope.content_type,
            inbox.envelope.text
        ]);
        let semantic_sha256 = match attachment {
            // Do not change ordinary-message identity or permit attachment
            // changes to alias a prior caption-only or different file receipt.
            None => digest(&text_semantics)?,
            Some(file) => digest(&serde_json::json!([
                "cma.bridge.discord-text-file.v1",
                text_semantics,
                file.attachment_id,
                file.filename,
                file.size_bytes,
                file.source_url_sha256
            ]))?,
        };
        Ok(Some(Self {
            source,
            key_sha256,
            semantic_sha256,
        }))
    }
    pub fn key_sha256(&self) -> &str {
        &self.key_sha256
    }
    pub fn semantic_sha256(&self) -> &str {
        &self.semantic_sha256
    }
    pub(super) fn require_source(&self, inbox: &StoredInbox) -> Result<(), CallbackError> {
        if self.source
            != PartitionSourcePin::for_inbox(inbox).map_err(|_| CallbackError::Payload)?
        {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }
}
