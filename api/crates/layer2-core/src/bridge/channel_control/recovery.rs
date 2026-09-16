use crate::bridge::ingress::InboxWrite;

use super::{ChannelControlCallback, ChannelControlError as Error, ChannelControlRequest};

/// Authenticated replay identity only. Current admission and reply observations
/// cannot invalidate complete prior acceptance. This borrowed input has no
/// conversion into a fresh control write, admission proof or reservation.
pub struct ChannelControlRecovery<'a> {
    inbox: &'a InboxWrite,
    callback: ChannelControlCallback,
    request: ChannelControlRequest,
}

impl<'a> ChannelControlRecovery<'a> {
    pub fn from_authenticated(inbox: &'a InboxWrite) -> Result<Self, Error> {
        if inbox.record().capabilities.is_none()
            || inbox.record().envelope.content_type != "text"
            || !inbox.record().envelope.attachments.is_empty()
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
        let callback = ChannelControlCallback::capture_inbox(inbox)?;
        Ok(Self {
            inbox,
            callback,
            request,
        })
    }

    pub fn inbox(&self) -> &InboxWrite {
        self.inbox
    }
    pub fn callback(&self) -> &ChannelControlCallback {
        &self.callback
    }
    pub fn request(&self) -> &ChannelControlRequest {
        &self.request
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.inbox
            .require_current(self.inbox.bridge())
            .map_err(|_| Error::AuthorityUnavailable)
    }
}
