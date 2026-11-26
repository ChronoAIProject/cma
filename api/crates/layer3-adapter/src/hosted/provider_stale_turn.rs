use super::*;
use crate::owner_protocol::current_authority;
use crate::stale_turn::{
    StaleTurnEvidence, StaleTurnHold, StaleTurnProducer, StaleTurnSeal, StaleTurnTarget,
    MAX_SEAL_BYTES,
};

const SEAL_DEADLINE: Duration = Duration::from_secs(3);

impl ProviderHostedControlTransport {
    /// The caller supplies the immutable digest only after qualifying the shipped image.
    pub fn with_qualified_stale_turn_image(mut self, digest: String) -> crate::Result<Self> {
        if !digest
            .strip_prefix("sha256:")
            .is_some_and(super::super::reply::valid_sha256)
        {
            return Err(AdapterError::Rejected(
                "invalid qualified stale-turn image digest".into(),
            ));
        }
        self.stale_turn_image = Some(digest);
        Ok(self)
    }

    pub(super) fn qualified_stale_turn_producer(&self) -> Option<&dyn StaleTurnProducer> {
        self.stale_turn_image.as_ref()?;
        (matches!(self.client, RuntimeDiscovery::AgentSandbox(_))
            && self.native_transport != NativeRuntimeTransport::Direct)
            .then_some(self)
    }

    async fn stale_turn_exchange(
        &self,
        target: &StaleTurnTarget,
        action: &str,
        max_bytes: usize,
    ) -> Result<(http::StatusCode, Vec<u8>), StaleTurnHold> {
        let Some(image_digest) = self.stale_turn_image.as_deref() else {
            return Err(StaleTurnHold::Unsupported);
        };
        let RuntimeDiscovery::AgentSandbox(client) = &self.client else {
            return Err(StaleTurnHold::Unsupported);
        };
        if self.native_transport == NativeRuntimeTransport::Direct {
            return Err(StaleTurnHold::Unsupported);
        }
        if !target.is_valid() {
            return Err(StaleTurnHold::Unconfirmed);
        }
        let authority = current_authority().ok_or(StaleTurnHold::Unconfirmed)?;
        let owner = authority.owner();
        if owner.agent_id != target.slot.0 || owner.epoch != target.writer_epoch {
            return Err(StaleTurnHold::Unconfirmed);
        }
        let exchange = async {
            authority
                .check()
                .await
                .map_err(|_| StaleTurnHold::Unconfirmed)?;
            let endpoint = self
                .endpoint(&target.runtime)
                .await
                .map_err(|_| StaleTurnHold::Unconfirmed)?;
            let (handle, binding) = endpoint
                .native_binding()
                .ok_or(StaleTurnHold::Unsupported)?;
            let identity = endpoint
                .native_identity()
                .ok_or(StaleTurnHold::Unconfirmed)?;
            if handle != &target.runtime
                || identity.sandbox_id.as_deref() != Some(target.sandbox_id.as_str())
            {
                return Err(StaleTurnHold::Unconfirmed);
            }
            let running = client
                .receiver_incarnation(handle, binding)
                .await
                .map_err(|_| StaleTurnHold::Unconfirmed)?;
            let observed_digest = running
                .image_id
                .rsplit_once('@')
                .map_or(running.image_id.as_str(), |(_, digest)| digest);
            if observed_digest != image_digest {
                return Err(StaleTurnHold::Unsupported);
            }
            let url = endpoint
                .stale_turn(target.slot.as_str(), action)
                .map_err(|_| StaleTurnHold::Unconfirmed)?;
            let body = serde_json::to_vec(target).map_err(|_| StaleTurnHold::Unconfirmed)?;
            let reply = self
                .native_http_for(
                    &endpoint,
                    http::Method::POST,
                    url,
                    body,
                    max_bytes,
                    (
                        native_pool::NativeHttpLane::Control,
                        RuntimeWorkloadClass::Continuity,
                    ),
                )
                .await
                .map_err(|_| StaleTurnHold::Unconfirmed)?;
            authority
                .check()
                .await
                .map_err(|_| StaleTurnHold::Unconfirmed)?;
            let status = reply.status;
            if status.is_success() {
                let bytes = reply
                    .recycle()
                    .await
                    .map_err(|_| StaleTurnHold::Unconfirmed)?;
                Ok((status, bytes))
            } else if matches!(
                status,
                http::StatusCode::NOT_FOUND | http::StatusCode::NOT_IMPLEMENTED
            ) {
                Err(StaleTurnHold::Unsupported)
            } else {
                Err(StaleTurnHold::Unconfirmed)
            }
        };
        tokio::time::timeout(SEAL_DEADLINE, exchange)
            .await
            .map_err(|_| StaleTurnHold::Unconfirmed)?
    }
}

#[async_trait::async_trait]
impl StaleTurnProducer for ProviderHostedControlTransport {
    async fn request_stale_turn_stop(&self, target: &StaleTurnTarget) -> Result<(), StaleTurnHold> {
        let (status, _) = self.stale_turn_exchange(target, "stop", 0).await?;
        if status == http::StatusCode::ACCEPTED {
            Ok(())
        } else {
            Err(StaleTurnHold::Unconfirmed)
        }
    }

    async fn observe_stale_turn_seal(
        &self,
        target: &StaleTurnTarget,
        max_evidence_bytes: usize,
    ) -> Result<StaleTurnEvidence, StaleTurnHold> {
        let (status, body) = self
            .stale_turn_exchange(target, "observe", MAX_SEAL_BYTES.min(max_evidence_bytes))
            .await?;
        if status != http::StatusCode::OK {
            return Err(StaleTurnHold::Unconfirmed);
        }
        let seal: StaleTurnSeal =
            serde_json::from_slice(&body).map_err(|_| StaleTurnHold::Unconfirmed)?;
        if !seal.matches(target) {
            return Err(StaleTurnHold::Unconfirmed);
        }
        Ok(StaleTurnEvidence {
            seal,
            bytes_read: body.len(),
        })
    }
}
