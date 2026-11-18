use super::{bounded, Authority, OwnershipError};
use layer3_adapter::owner_protocol::{
    OwnershipFailure, OwnershipFailureReason, OwnershipPhase, TransportAuthority, TransportOwner,
};
use layer3_adapter::AdapterError;
use std::time::Duration;

#[async_trait::async_trait]
impl TransportAuthority for Authority {
    fn register_receiver(
        &self,
        receiver: std::sync::Arc<dyn layer3_adapter::owner_protocol::ReceiverRevocation>,
    ) -> layer3_adapter::Result<()> {
        let mut receivers = self.receivers.lock().unwrap();
        if !self.is_current() || receivers.len() >= 64 {
            return Err(stale());
        }
        receivers.insert(receiver.identity(), receiver);
        Ok(())
    }
    fn is_current(&self) -> bool {
        Authority::is_current(self)
    }
    fn owner(&self) -> TransportOwner {
        TransportOwner {
            protocol: self.token.protocol,
            agent_id: self.token.agent_id.to_string(),
            instance_id: self.token.instance_id.clone(),
            epoch: self.token.epoch,
        }
    }

    async fn check(&self) -> layer3_adapter::Result<()> {
        self.check_for_phase(OwnershipPhase::Renew).await
    }

    async fn check_for_phase(&self, phase: OwnershipPhase) -> layer3_adapter::Result<()> {
        if !self.is_current() {
            return Err(stale());
        }
        let store = self.store.as_ref().ok_or_else(stale)?;
        if let Err(error) = bounded(store.check(&self.token)).await {
            self.receiver_error("receiver_store_check", &error);
            self.record_failure(OwnershipFailure::new(phase, failure_reason(&error)))
                .await;
            self.cancel_receiver();
            return Err(stale());
        }
        if self.is_current() {
            Ok(())
        } else {
            Err(stale())
        }
    }

    async fn renew_for_challenge(&self) -> layer3_adapter::Result<()> {
        if !self.is_current() {
            return Err(stale());
        }
        let store = self.store.as_ref().ok_or_else(stale)?;
        let started = tokio::time::Instant::now();
        let result = async {
            let membership = bounded(store.membership()).await.inspect_err(|error| {
                self.receiver_error("receiver_membership", error);
            })?;
            let lease = bounded(store.renew(&self.token))
                .await
                .inspect_err(|error| {
                    self.receiver_error("receiver_store_renew", error);
                })?;
            if lease.token != self.token {
                self.receiver_error("receiver_same_token", &OwnershipError::Stale);
                return Err(OwnershipError::Stale);
            }
            let remaining = lease
                .expires_at_ms
                .checked_sub(membership.now_ms)
                .filter(|remaining| *remaining > 0)
                .ok_or_else(|| {
                    self.receiver_error("receiver_store_deadline", &OwnershipError::Stale);
                    OwnershipError::Stale
                })?;
            self.renew_until(started + Duration::from_millis(remaining as u64))
                .inspect_err(|error| self.receiver_error("receiver_local_renew", error))
        }
        .await;
        if let Err(error) = &result {
            self.record_failure(OwnershipFailure::new(
                OwnershipPhase::Renew,
                failure_reason(error),
            ))
            .await;
            self.cancel_receiver();
        }
        result.map_err(|_| stale())
    }

    async fn record_failure(&self, failure: OwnershipFailure) {
        self.receiver_failure.get_or_init(|| {
            use sha2::{Digest, Sha256};
            let agent_sha256 = format!("{:x}", Sha256::digest(self.token.agent_id.as_str().as_bytes()));
            let instance_sha256 = format!("{:x}", Sha256::digest(self.token.instance_id.as_bytes()));
            tracing::warn!(%agent_sha256, %instance_sha256, epoch = self.token.epoch,
                phase = failure.phase.as_str(), reason = failure.reason.as_str(),
                http_status = failure.http_status, protocol = failure.protocol, lease_ms = failure.lease_ms,
                "Receiver ownership negotiation failed");
            failure
        });
    }

    async fn revoked(&self) {
        Authority::revoked(self).await;
    }
    fn revoke(&self) {
        self.cancel_receiver();
    }
}

impl Authority {
    fn receiver_error(&self, stage: &'static str, error: &OwnershipError) {
        use sha2::{Digest, Sha256};
        let agent_sha256 = format!(
            "{:x}",
            Sha256::digest(self.token.agent_id.as_str().as_bytes())
        );
        let instance_sha256 = format!("{:x}", Sha256::digest(self.token.instance_id.as_bytes()));
        tracing::warn!(stage, %agent_sha256, %instance_sha256, epoch = self.token.epoch,
            reason = failure_reason(error).as_str(), "Receiver ownership operation failed");
    }

    /// Retain every contacted receiver until all revocations succeed. A cancelled
    /// or ambiguous confirmation may have installed authority at the runtime.
    pub async fn revoke_transports(&self) -> Result<(), super::OwnershipError> {
        self.revoke();
        let receivers: Vec<_> = self.receivers.lock().unwrap().values().cloned().collect();
        for receiver in receivers {
            receiver.revoke().await.map_err(|_| {
                super::OwnershipError::Unavailable("receiving runtime revocation failed".into())
            })?;
        }
        self.receivers.lock().unwrap().clear();
        Ok(())
    }
}

fn stale() -> AdapterError {
    AdapterError::Rejected("Agent Manager transport ownership is stale or unavailable".into())
}

fn failure_reason(error: &OwnershipError) -> OwnershipFailureReason {
    match error {
        OwnershipError::Unavailable(detail)
            if detail == "ownership operation exceeded its deadline" =>
        {
            OwnershipFailureReason::Deadline
        }
        OwnershipError::Unavailable(_) => OwnershipFailureReason::TransportUnavailable,
        OwnershipError::IncompatibleProtocol => OwnershipFailureReason::ProtocolMismatch,
        OwnershipError::Stale | OwnershipError::Invalid(_) => OwnershipFailureReason::AuthorityLost,
    }
}
