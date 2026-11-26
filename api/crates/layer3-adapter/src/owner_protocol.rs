//! Internal Agent Manager ownership negotiation, independent of public APIs.
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc};

#[path = "hosted/provider_ownership_failure.rs"]
mod failure;
pub(crate) use failure::OwnershipExchangeFailure;
pub use failure::{OwnershipFailure, OwnershipFailureReason, OwnershipPhase};

#[path = "hosted/provider_ownership_evidence.rs"]
mod evidence;
pub use evidence::{FirstOwnershipFailure, OwnershipEvidence, OwnershipReceiver};
#[path = "receiver_settlement.rs"]
mod settlement;
pub use settlement::{ReceiverSettlementFailure, ReceiverSettlementWatch};

#[async_trait::async_trait]
pub trait ReceiverRevocation: Send + Sync {
    fn identity(&self) -> String;
    fn sandbox_id(&self) -> Option<&str> {
        None
    }
    async fn revoke(&self) -> crate::Result<()>;
    /// Compatibility can satisfy one cleanup obligation only when the owner
    /// has closed confirmation admission and this receiver could not install it.
    async fn settle(
        &self,
        _custody: ReceiverConfirmationCustody,
    ) -> crate::Result<ReceiverCleanup> {
        self.revoke().await?;
        Ok(ReceiverCleanup::Acknowledged)
    }

    async fn settle_typed(
        &self,
        custody: ReceiverConfirmationCustody,
        outcome: &ReceiverSettlementWatch,
    ) -> Result<ReceiverCleanup, ReceiverSettlementFailure> {
        self.settle(custody).await.map_err(|_| {
            outcome.record(ReceiverSettlementFailure::Unclassified);
            ReceiverSettlementFailure::Unclassified
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverConfirmationCustody {
    NeverSent,
    ReceiverObserved,
    PossiblyDelivered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverCleanup {
    Acknowledged,
    NoReceiver,
}

#[async_trait::async_trait]
pub trait TransportAuthority: Send + Sync {
    fn retry_evidence(&self) -> Option<Arc<OwnershipEvidence>> {
        None
    }
    fn owner(&self) -> TransportOwner;
    fn is_current(&self) -> bool;
    fn register_receiver(&self, receiver: Arc<dyn ReceiverRevocation>) -> crate::Result<()>;
    /// A successful challenge response contradicts a receiver-less image proof.
    fn receiver_observed(&self, _identity: &str) {}
    /// Record custody before dispatch, even if confirmation is then cancelled
    /// or its reply is lost. A retired authority must refuse this transition.
    fn begin_receiver_confirmation(&self, identity: &str) -> crate::Result<()>;
    /// Advisory observation only, after an exact-owner confirmation succeeds.
    fn receiver_confirmed(&self, _identity: &str) {}
    async fn check(&self) -> crate::Result<()>;
    async fn check_for_phase(&self, _phase: OwnershipPhase) -> crate::Result<()> {
        self.check().await
    }
    /// Must renew the exact durable owner only after receiving a fresh runtime
    /// challenge. Never confirm a challenge when this call fails or is cancelled.
    async fn renew_for_challenge(&self) -> crate::Result<()>;
    /// Retain the first safe cause before signalling cancellation of the caller.
    async fn record_failure(&self, failure: OwnershipFailure);
    /// Finish a bounded diagnostic attempt before cancellation. Dropping the
    /// callback must still revoke, including when a live link aborts renewal.
    async fn record_failure_and_revoke(&self, failure: OwnershipFailure) {
        let _revoke = RevokeOnDrop(self);
        self.record_failure(failure).await;
    }
    async fn revoked(&self);
    fn revoke(&self);
}

struct RevokeOnDrop<'a, T: TransportAuthority + ?Sized>(&'a T);

impl<T: TransportAuthority + ?Sized> Drop for RevokeOnDrop<'_, T> {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

tokio::task_local! {
    static AUTHORITY: Arc<dyn TransportAuthority>;
    static INSPECTION_FAILURE: Box<dyn Fn(InspectionRefusal) + Send + Sync>;
}

/// Retain the receiver failure from this read even when it cancels the caller.
pub async fn observe_inspection_failure<F: Future>(
    observed: impl Fn(InspectionRefusal) + Send + Sync + 'static,
    read: F,
) -> F::Output {
    INSPECTION_FAILURE.scope(Box::new(observed), read).await
}

/// Evidence from this exact receiver exchange, retained before scope cancellation.
pub struct InspectionRefusal {
    pub owner: TransportOwner,
    pub receiver: OwnershipReceiver,
    pub failure: OwnershipFailure,
}

pub(crate) fn record_inspection_refusal(
    owner: TransportOwner,
    receiver: OwnershipReceiver,
    failure: OwnershipFailure,
) {
    if failure.reason != OwnershipFailureReason::AuthorityLost {
        let _ = INSPECTION_FAILURE.try_with(|observed| {
            observed(InspectionRefusal {
                owner,
                receiver,
                failure,
            });
        });
    }
}

pub fn current_authority() -> Option<Arc<dyn TransportAuthority>> {
    AUTHORITY.try_with(Arc::clone).ok()
}

pub async fn scope<F: Future>(authority: Arc<dyn TransportAuthority>, operation: F) -> F::Output {
    AUTHORITY.scope(authority, operation).await
}

pub fn current_header() -> crate::Result<Option<String>> {
    current_authority()
        .map(|authority| authority.owner().header())
        .transpose()
}

pub const OWNER_PROTOCOL_VERSION: u32 = 1;
pub const OWNER_HEADER: &str = "x-cma-manager-ownership";
/// Starts at the receiver before the manager's subsequent 15-second Mongo
/// renewal. Retained receivers advertising the old 20-second lease are refused.
pub const RECEIVER_LEASE_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportOwner {
    pub protocol: u32,
    pub agent_id: String,
    pub instance_id: String,
    pub epoch: u64,
}

impl TransportOwner {
    pub fn validate(&self) -> crate::Result<()> {
        if self.protocol != OWNER_PROTOCOL_VERSION
            || self.epoch == 0
            || self.agent_id.is_empty()
            || self.agent_id.len() > 128
            || self.instance_id.is_empty()
            || self.instance_id.len() > 128
        {
            return Err(crate::AdapterError::Rejected(
                "invalid transport ownership token".into(),
            ));
        }
        Ok(())
    }

    pub fn header(&self) -> crate::Result<String> {
        use base64::Engine;
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|_| crate::AdapterError::Rejected("ownership token encoding failed".into()))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn from_header(value: &str) -> crate::Result<Self> {
        use base64::Engine;
        let invalid = || crate::AdapterError::Rejected("invalid transport ownership header".into());
        if value.len() > 1_024 {
            return Err(invalid());
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| invalid())?;
        let token: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        token.validate()?;
        Ok(token)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerChallenge {
    pub protocol: u32,
    pub nonce: String,
    pub valid_for_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerConfirmation {
    pub owner: TransportOwner,
    pub nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerAcknowledgement {
    pub owner: TransportOwner,
    pub protocol: u32,
}

#[cfg(test)]
#[path = "owner_protocol_test_support.rs"]
pub(crate) mod test_support;
