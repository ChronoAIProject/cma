use crate::bridge::{
    ingress::{ChannelControlRefusal, InboxAdmission, InboxWrite},
    partition::{ChannelConversationKind, NormalizedPartitionSource, ThreadEvidence},
};

use super::{ChannelControlError as Error, ChannelControlRequest};

/// An actual authenticated callback, never deserializable retained data. This
/// wrapper preserves the original signature/binding deadline and provider proof.
pub struct ChannelControlWrite {
    inbox: InboxWrite,
    request: ChannelControlRequest,
    source: NormalizedPartitionSource,
}

impl ChannelControlWrite {
    pub fn from_verified(inbox: InboxWrite) -> Result<Self, Error> {
        inbox
            .require_current(inbox.bridge())
            .map_err(|_| Error::AuthorityUnavailable)?;
        // The private classifier is set only after authenticated raw text/human
        // observation. A generic normalized payload cannot bypass that boundary.
        if inbox.control_refusal() != Some(ChannelControlRefusal::ChannelControlsDisabled)
            || inbox.record().admission != InboxAdmission::Accepted
            || inbox.platform_redelivery().is_none()
            || inbox.record().envelope.content_type != "text"
            || !inbox.record().envelope.attachments.is_empty()
            || inbox.record().text_attachment.is_some()
        {
            return Err(Error::CapabilityUnsupported);
        }
        let request = ChannelControlRequest::parse(
            inbox
                .record()
                .envelope
                .text
                .as_deref()
                .ok_or(Error::Invalid)?,
        )?;
        let source = NormalizedPartitionSource::from_stored(inbox.record(), inbox.bridge())
            .map_err(|_| Error::SourceUnavailable)?;
        if source.provider_message_id().is_none()
            || matches!(source.thread(), ThreadEvidence::Unknown)
            || source.conversation_kind() == ChannelConversationKind::Unknown
        {
            return Err(Error::CapabilityUnsupported);
        }
        Ok(Self {
            inbox,
            request,
            source,
        })
    }

    pub fn inbox(&self) -> &InboxWrite {
        &self.inbox
    }
    /// Returns the original authenticated write for the existing durable
    /// disabled-refusal path; neither identity nor deadline is refreshed.
    pub fn into_inbox(self) -> InboxWrite {
        self.inbox
    }
    pub fn request(&self) -> &ChannelControlRequest {
        &self.request
    }
    pub fn source(&self) -> &NormalizedPartitionSource {
        &self.source
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.inbox
            .require_current(self.inbox.bridge())
            .map_err(|_| Error::AuthorityUnavailable)
    }
}
