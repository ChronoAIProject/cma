//! Deployment-wide opt-in for new attachment admissions; ordinary input remains
//! available when disabled. Already accepted commands keep their original input.
use layer2_core::bridge::{
    ingress::{
        AuthenticatedCallbackPayload, CallbackCapabilityExtractor, CallbackError,
        VerifiedTextAttachment,
    },
    partition::{CapabilityContract, ChannelEventKind, PlatformCapabilityObservation},
};

pub(in crate::bridge) struct TextAdmission<'a> {
    pub inner: &'a dyn CallbackCapabilityExtractor,
    pub enabled: bool,
}

impl CallbackCapabilityExtractor for TextAdmission<'_> {
    fn contract(&self) -> CapabilityContract {
        self.inner.contract()
    }
    fn observe(
        &self,
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<PlatformCapabilityObservation, CallbackError> {
        let mut facts = self.inner.observe(payload)?;
        if !self.enabled && facts.event_kind == ChannelEventKind::DiscordTextAttachment {
            facts.event_kind = ChannelEventKind::AttachmentMessage;
        }
        Ok(facts)
    }
    fn text_attachment(
        &self,
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<Option<VerifiedTextAttachment>, CallbackError> {
        if self.enabled {
            self.inner.text_attachment(payload)
        } else {
            Ok(None)
        }
    }
}
