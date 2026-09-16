//! Recovery-first control ingress. Local runtime availability never grants
//! source authority, and a deployment selector cannot prove owner routing.
use super::*;
use crate::{
    config::{bridge_flags::BridgeFlag, BridgeFeatureFlags},
    store::{MongoCallbackInbox, MongoChannelControlStore},
};
use layer2_core::bridge::{
    channel_control::{
        ChannelControlError, ChannelControlProof, ChannelControlRequest, ChannelControlReservation,
        ChannelControlWrite,
    },
    ingress::{ChannelControlRefusal, InboxAdmission, InboxWrite},
    invoke::BridgeInvokeService,
};

#[async_trait::async_trait]
pub(crate) trait ChannelControlRuntimeEligibility: Send + Sync {
    /// Require the exact operation, Agent incarnation, runtime generation and
    /// conversation in this current proof. This supplies availability only.
    async fn require_available(
        &self,
        proof: &ChannelControlProof,
    ) -> Result<(), ChannelControlError>;
}

struct UnavailableRuntime;

#[async_trait::async_trait]
impl ChannelControlRuntimeEligibility for UnavailableRuntime {
    async fn require_available(&self, _: &ChannelControlProof) -> Result<(), ChannelControlError> {
        // No production implementation yet proves selected-command claim
        // routing, compatible ownership/handoff or provider control semantics.
        Err(ChannelControlError::Unavailable)
    }
}

pub(crate) struct ChannelControlIngress {
    callbacks: Arc<MongoCallbackInbox>,
    store: Arc<MongoChannelControlStore>,
    invokes: BridgeInvokeService,
    flags: BridgeFeatureFlags,
    runtime: Arc<dyn ChannelControlRuntimeEligibility>,
}

impl ChannelControlIngress {
    pub(crate) fn production(
        callbacks: Arc<MongoCallbackInbox>,
        store: Arc<MongoChannelControlStore>,
        invokes: BridgeInvokeService,
        flags: BridgeFeatureFlags,
    ) -> Self {
        Self {
            callbacks,
            store,
            invokes,
            flags,
            runtime: Arc::new(UnavailableRuntime),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_runtime(
        mut self,
        runtime: Arc<dyn ChannelControlRuntimeEligibility>,
    ) -> Self {
        self.runtime = runtime;
        self
    }

    pub(crate) async fn accept(
        &self,
        write: InboxWrite,
    ) -> Result<CallbackAcceptance, CallbackError> {
        if write.control_refusal() != Some(ChannelControlRefusal::ChannelControlsDisabled)
            || write.record().admission != InboxAdmission::Accepted
            || write.record().envelope.content_type != "text"
            || !write.record().envelope.attachments.is_empty()
            || write.platform_redelivery().is_none()
            || write
                .record()
                .envelope
                .text
                .as_deref()
                .is_none_or(|text| ChannelControlRequest::parse(text).is_err())
        {
            return self.callbacks.accept(write).await;
        }
        let deadline = write.deadline();
        tokio::time::timeout_at(deadline, async {
            let write = ChannelControlWrite::from_verified(write).map_err(callback_error)?;
            // Old Message/refusal dispositions and accepted-v2 provider aliases
            // win before fresh configuration, source or runtime observations.
            if let Some(existing) = self
                .store
                .recover_acceptance(&write)
                .await
                .map_err(callback_error)?
            {
                return Ok(existing.acceptance);
            }
            if !self.flags.enabled(BridgeFlag::ControlAdmission) {
                return self.callbacks.accept(write.into_inbox()).await;
            }
            let proof = self
                .store
                .admission_proof(write, &self.invokes)
                .await
                .map_err(callback_error)?;
            self.runtime
                .require_available(&proof)
                .await
                .map_err(callback_error)?;
            proof.require_current().map_err(callback_error)?;
            let request = ChannelControlReservation::prepare(proof).map_err(callback_error)?;
            self.store
                .admit(request)
                .await
                .map(|result| result.acceptance)
                .map_err(callback_error)
        })
        .await
        .map_err(|_| CallbackError::Unavailable)?
    }
}

fn callback_error(error: ChannelControlError) -> CallbackError {
    match error {
        ChannelControlError::Invalid => CallbackError::Payload,
        ChannelControlError::Conflict => CallbackError::ReplayConflict,
        // Ineligible, missing or uncertain evidence must not become a durable
        // source denial or an ordinary Message. No new refusal schema is added.
        _ => CallbackError::Unavailable,
    }
}
