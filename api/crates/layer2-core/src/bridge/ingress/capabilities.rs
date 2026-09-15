use sha2::{Digest, Sha256};

use crate::bridge::partition::{
    CapabilityContract, NormalizedPartitionSource, PartitionAdmissionDecision, PartitionError,
    PartitionRefusal, PlatformCapabilityObservation, StoredPartitionObservation,
};

use super::{CallbackError, InboxAdmission, SignedCallback, StoredInbox};

/// Only SignedCallback constructs this view after cryptographic body matching.
/// A view is historical transport evidence, never allocation/invoke authority.
pub struct AuthenticatedCallbackPayload<'a> {
    body: &'a [u8],
    record: &'a StoredInbox,
}

impl AuthenticatedCallbackPayload<'_> {
    pub fn body(&self) -> &[u8] {
        self.body
    }
    pub fn record(&self) -> &StoredInbox {
        self.record
    }
}

/// Trusted platform boundary, with no platform-specific logic in Layer 2.
/// The extractor must cross-check raw sender/chat identity with the signed
/// normalized envelope and leave absent/unsupported capability facts Unknown.
pub trait CallbackCapabilityExtractor {
    fn contract(&self) -> CapabilityContract;
    fn observe(
        &self,
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<PlatformCapabilityObservation, CallbackError>;
}

impl SignedCallback {
    pub fn classify_capabilities(
        mut self,
        body: &[u8],
        extractor: &dyn CallbackCapabilityExtractor,
    ) -> Result<Self, CallbackError> {
        if tokio::time::Instant::now() >= self.deadline
            || body.len() > 1_048_576
            || format!("{:x}", Sha256::digest(body)) != self.record.attestation.body_sha256
            || self.record.capabilities.is_some()
        {
            return Err(CallbackError::Authentication);
        }
        let facts = extractor.observe(&AuthenticatedCallbackPayload {
            body,
            record: &self.record,
        })?;
        // NyxID's route-level conversation type can describe a wildcard route.
        // The trusted extractor uses actual authenticated platform chat facts.
        self.record.envelope.conversation_type = facts.conversation.as_wire().into();
        // This local provisional value permits the full scoped sender/chat gates
        // to run. Only the final typed decision is eligible for durable binding.
        self.record.admission = InboxAdmission::Accepted;
        let source =
            NormalizedPartitionSource::from_observation(&self.record, &self.bridge, facts.clone())
                .map_err(|_| CallbackError::Payload)?;
        let config = &self
            .bridge
            .pinned(&self.record.pin)
            .map_err(|_| CallbackError::BindingUnavailable)?
            .config;
        self.record.admission = match source.admit(config) {
            Ok(PartitionAdmissionDecision::Admitted { .. }) => InboxAdmission::Accepted,
            Ok(PartitionAdmissionDecision::Refused { reason }) => {
                InboxAdmission::PartitionDenied { reason }
            }
            Err(PartitionError::InvalidPolicy(_)) => InboxAdmission::PartitionDenied {
                reason: PartitionRefusal::ConfigurationUnsupported,
            },
            Err(_) => return Err(CallbackError::Payload),
        };
        self.record.capabilities = Some(StoredPartitionObservation::from_observation(
            &self.record,
            &facts,
            extractor.contract(),
        ));
        if tokio::time::Instant::now() >= self.deadline {
            return Err(CallbackError::Authentication);
        }
        Ok(self)
    }
}
