use super::{bounded, Authority, OwnershipError};
use layer3_adapter::owner_protocol::{
    OwnershipFailure, OwnershipFailureReason, OwnershipPhase, TransportAuthority, TransportOwner,
};
use layer3_adapter::AdapterError;
use std::time::Duration;

#[path = "receiver_diagnostics.rs"]
mod diagnostics;
use diagnostics::{emit, Event};

#[async_trait::async_trait]
impl TransportAuthority for Authority {
    fn retry_evidence(
        &self,
    ) -> Option<std::sync::Arc<layer3_adapter::owner_protocol::OwnershipEvidence>> {
        self.retry_evidence.get().cloned()
    }
    fn register_receiver(
        &self,
        receiver: std::sync::Arc<dyn layer3_adapter::owner_protocol::ReceiverRevocation>,
    ) -> layer3_adapter::Result<()> {
        let mut receivers = self.receivers.lock().unwrap();
        if !self.is_current() || receivers.len() >= 64 {
            return Err(stale());
        }
        let identity = receiver.identity();
        self.confirmed_receivers.lock().unwrap().remove(&identity);
        receivers.insert(identity, receiver);
        Ok(())
    }
    fn receiver_confirmed(&self, identity: &str) {
        self.confirm_receiver_binding(identity);
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
            self.record_failure_and_revoke(OwnershipFailure::new(phase, failure_reason(&error)))
                .await;
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
            self.record_failure_and_revoke(OwnershipFailure::new(
                OwnershipPhase::Renew,
                failure_reason(error),
            ))
            .await;
        }
        result.map_err(|_| stale())
    }

    async fn record_failure(&self, failure: OwnershipFailure) {
        if let Some(evidence) = self.retry_evidence.get() {
            evidence.record_unknown(self.owner(), failure);
        }
        let Some(store) = self
            .store
            .as_ref()
            .filter(|store| store.receiver_diagnostics_enabled())
        else {
            return;
        };
        let first = *self.receiver_failure.get_or_init(|| {
            emit(&self.token, Event::First(failure));
            failure
        });
        let mut recording = self
            .receiver_failure_recording
            .get_or_init(|| {
                let (finished, receiver) = tokio::sync::watch::channel(false);
                let store = store.clone();
                let token = self.token.clone();
                // Expiry or another cancellation can discard this caller. The
                // bounded diagnostic write owns no execution authority and survives it.
                tokio::spawn(async move {
                    let result = bounded(store.record_receiver_failure(&token, first)).await;
                    let event = match result {
                        Ok(()) => Event::Recorded,
                        Err(error) => Event::Unconfirmed(failure_reason(&error)),
                    };
                    emit(&token, event);
                    finished.send_replace(true);
                });
                receiver
            })
            .clone();
        if recording.wait_for(|finished| *finished).await.is_err() {
            self.receiver_error(
                "receiver_diagnostic_task",
                &OwnershipError::Unavailable(
                    "diagnostic task ended before recording its outcome".into(),
                ),
            );
        }
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
        emit(
            &self.token,
            Event::Operation {
                stage,
                reason: failure_reason(error),
            },
        );
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
