use super::{bounded, Authority, OwnershipError};
use layer3_adapter::owner_protocol::{
    OwnershipFailure, OwnershipFailureReason, OwnershipPhase, ReceiverCleanup,
    ReceiverConfirmationCustody, ReceiverRevocation, ReceiverSettlementFailure, TransportAuthority,
    TransportOwner,
};
use layer3_adapter::AdapterError;
use std::{sync::Arc, time::Duration};

use super::receiver_diagnostics::{emit, Event};

#[derive(Clone)]
pub(super) struct RegisteredReceiver {
    revocation: Arc<dyn ReceiverRevocation>,
    confirmation: ReceiverConfirmationCustody,
}

#[async_trait::async_trait]
impl TransportAuthority for Authority {
    fn retry_evidence(
        &self,
    ) -> Option<std::sync::Arc<layer3_adapter::owner_protocol::OwnershipEvidence>> {
        self.retry_evidence.get().cloned()
    }
    fn register_receiver(
        &self,
        receiver: Arc<dyn ReceiverRevocation>,
    ) -> layer3_adapter::Result<()> {
        let mut receivers = self.receivers.lock().unwrap();
        if !self.is_current() {
            return Err(stale());
        }
        let identity = receiver.identity();
        if let Some(registered) = receivers.get_mut(&identity) {
            // Preserve the captured revoker once we know this receiver may
            // exist. A later registration cannot replace stronger custody.
            if registered.confirmation == ReceiverConfirmationCustody::NeverSent {
                registered.revocation = receiver;
            }
        } else {
            if receivers.len() >= 64 {
                return Err(stale());
            }
            receivers.insert(
                identity.clone(),
                RegisteredReceiver {
                    revocation: receiver,
                    confirmation: ReceiverConfirmationCustody::NeverSent,
                },
            );
        }
        if let Some(sandbox_id) = receivers[&identity].revocation.sandbox_id() {
            self.observe_diagnostic_sandbox(sandbox_id);
        }
        self.confirmed_receivers.lock().unwrap().remove(&identity);
        Ok(())
    }
    fn receiver_observed(&self, identity: &str) {
        if let Some(receiver) = self.receivers.lock().unwrap().get_mut(identity) {
            if receiver.confirmation == ReceiverConfirmationCustody::NeverSent {
                receiver.confirmation = ReceiverConfirmationCustody::ReceiverObserved;
            }
        }
    }
    fn begin_receiver_confirmation(&self, identity: &str) -> layer3_adapter::Result<()> {
        let mut receivers = self.receivers.lock().unwrap();
        if !self.is_current() {
            return Err(stale());
        }
        let receiver = receivers.get_mut(identity).ok_or_else(stale)?;
        receiver.confirmation = ReceiverConfirmationCustody::PossiblyDelivered;
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
            let lease = async {
                if self.policy == super::MaintenancePolicy::ContainFailures {
                    self.renew_store_owned().await
                } else {
                    bounded(store.renew(&self.token)).await
                }
            }
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
            emit(
                &self.token,
                &self.receiver_correlation(),
                Event::First(failure),
            );
            failure
        });
        let mut recording = self
            .receiver_failure_recording
            .get_or_init(|| {
                let (finished, receiver) = tokio::sync::watch::channel(false);
                let store = store.clone();
                let token = self.token.clone();
                let fields = self.receiver_correlation();
                // Expiry or another cancellation can discard this caller. The
                // bounded diagnostic write owns no execution authority and survives it.
                tokio::spawn(async move {
                    let result = bounded(store.record_receiver_failure(&token, first)).await;
                    let event = match result {
                        Ok(()) => Event::Recorded,
                        Err(error) => Event::Unconfirmed(failure_reason(&error)),
                    };
                    emit(&token, &fields, event);
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
    pub(super) fn settlement_error(&self) -> Option<OwnershipError> {
        self.settlement_watch.terminal().map(settlement_error)
    }

    fn receiver_error(&self, stage: &'static str, error: &OwnershipError) {
        emit(
            &self.token,
            &self.receiver_correlation(),
            Event::Operation {
                stage,
                reason: failure_reason(error),
            },
        );
    }

    /// Retain every contacted receiver until all revocations succeed. A cancelled
    /// or ambiguous confirmation may have installed authority at the runtime.
    pub async fn revoke_transports(&self) -> Result<(), super::OwnershipError> {
        self.settle_transports(false).await
    }

    /// Run after closing local admission, joining owned work, and detaching
    /// owner sessions. A negative receiver proof does not provide quiescence.
    pub async fn revoke_transports_after_local_drain(&self) -> Result<(), super::OwnershipError> {
        self.settle_transports(true).await
    }

    async fn settle_transports(
        &self,
        after_local_drain: bool,
    ) -> Result<(), super::OwnershipError> {
        // Close confirmation admission before taking the same mutex used to
        // record a possible send. A racing send is either captured or refused.
        self.revoke();
        if after_local_drain {
            self.join_local_work().await?;
        }
        let receivers: Vec<_> = self
            .receivers
            .lock()
            .unwrap()
            .iter()
            .map(|(identity, receiver)| (identity.clone(), receiver.clone()))
            .collect();
        for (identity, receiver) in receivers {
            let result = if self.policy == super::MaintenancePolicy::ContainFailures {
                let custody = if after_local_drain {
                    receiver.confirmation
                } else {
                    ReceiverConfirmationCustody::PossiblyDelivered
                };
                receiver
                    .revocation
                    .settle_typed(custody, &self.settlement_watch)
                    .await
                    .map_err(|failure| {
                        self.settlement_watch.record(failure);
                        settlement_error(failure)
                    })
            } else if after_local_drain {
                receiver
                    .revocation
                    .settle(receiver.confirmation)
                    .await
                    .map_err(|_| unsettled())
            } else {
                receiver
                    .revocation
                    .revoke()
                    .await
                    .map(|()| ReceiverCleanup::Acknowledged)
                    .map_err(|_| unsettled())
            };
            match result {
                Ok(ReceiverCleanup::Acknowledged) => {
                    // Confirmation admission is closed. A matching runtime ACK
                    // settles this token permanently, even if a sibling or
                    // durable release still needs another attempt.
                    self.receivers.lock().unwrap().remove(&identity);
                    self.confirmed_receivers.lock().unwrap().remove(&identity);
                }
                Ok(ReceiverCleanup::NoReceiver)
                    if receiver.confirmation == ReceiverConfirmationCustody::NeverSent =>
                {
                    // Keep only negative evidence in the retry set. Its
                    // incumbent runtime must still be proven on the next attempt.
                }
                Err(error) => {
                    self.log_receiver_cleanup_failure(&identity, after_local_drain, &error);
                    return Err(error);
                }
                Ok(ReceiverCleanup::NoReceiver) => {
                    let error = if self.policy == super::MaintenancePolicy::ContainFailures {
                        self.settlement_watch
                            .record(ReceiverSettlementFailure::Invalid);
                        settlement_error(ReceiverSettlementFailure::Invalid)
                    } else {
                        unsettled()
                    };
                    self.log_receiver_cleanup_failure(&identity, after_local_drain, &error);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

fn unsettled() -> OwnershipError {
    OwnershipError::Unavailable("receiving runtime revocation failed".into())
}

fn settlement_error(failure: ReceiverSettlementFailure) -> OwnershipError {
    match failure {
        ReceiverSettlementFailure::Deadline | ReceiverSettlementFailure::Unavailable => unsettled(),
        ReceiverSettlementFailure::IncompatibleProtocol => OwnershipError::IncompatibleProtocol,
        ReceiverSettlementFailure::AuthorityLost => OwnershipError::Stale,
        ReceiverSettlementFailure::Invalid | ReceiverSettlementFailure::Unclassified => {
            OwnershipError::Invalid("receiver settlement validation failed".into())
        }
    }
}

fn stale() -> AdapterError {
    AdapterError::Rejected("Agent Manager transport ownership is stale or unavailable".into())
}

pub(super) fn failure_reason(error: &OwnershipError) -> OwnershipFailureReason {
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

#[cfg(test)]
#[path = "receiver_custody_tests.rs"]
mod tests;
