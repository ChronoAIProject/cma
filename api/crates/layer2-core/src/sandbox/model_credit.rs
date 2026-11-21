//! A funded phase requires both an authoritative reservation and a gateway
//! that can enforce its limit against the credential selected for dispatch.

use std::num::NonZeroU64;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

mod agent;
pub(in crate::sandbox) use agent::FundedAgentMessage;
pub use agent::{AgentModelGateway, AgentTurnFunding, ModelPhaseRequest};

/// No deployed adapter currently certifies the payer and bounds each phase.
/// Existing integrations may continue using the default, unfenced path.
pub(super) fn require_available(required: bool) -> Result<(), super::AgentOperationError> {
    if required {
        return Err(super::AgentOperationError::ModelFundingUnsupported);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialSource {
    Managed,
    Own,
}

/// Facts from trusted credential selection. No credential values cross this boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialBinding {
    pub source: CredentialSource,
    pub generation: NonZeroU64,
    pub route: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayerBinding {
    pub credential: CredentialBinding,
    pub payer: String,
    pub pool: String,
}

/// The caller uses the durable turn ID and monotonically advancing phase.
/// Retries and reconciliation retain this identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhaseId {
    pub turn: String,
    pub phase: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub id: PhaseId,
    pub binding: PayerBinding,
    pub limit: NonZeroU64,
    pub request_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// The model gateway proved the charge and all preceding effects.
    Finished { spent: u64 },
    /// The model gateway proved that no work or charge started.
    NotStarted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhaseState {
    Reserved,
    InFlight,
    Settled(Effect),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub reservation: Reservation,
    pub version: NonZeroU64,
    pub state: PhaseState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FundingError {
    #[error("guaranteed model funding is unsupported")]
    Unsupported,
    #[error("model funding is insufficient")]
    Insufficient,
    #[error("model funding is temporarily unavailable")]
    Unavailable,
    #[error("model funding can no longer begin")]
    Expired,
    #[error("model funding no longer matches the selected credential")]
    BindingChanged,
    #[error("model funding checkpoint changed")]
    CheckpointChanged,
    #[error("model work requires reconciliation")]
    ReconciliationRequired,
    #[error("model funding continuation is forbidden")]
    Forbidden,
}

/// The shared authority owns the pool and durable checkpoint together. Each
/// compare-and-set checks the full expected checkpoint atomically with any
/// change to the reservation or settled charge.
#[async_trait]
pub trait ReservationAuthority: Send + Sync {
    async fn reserve_first(&self, request: &Reservation) -> Result<Checkpoint, FundingError>;
    async fn read(&self, id: &PhaseId) -> Result<Checkpoint, FundingError>;
    /// Fence a reserved request only while eligible. Expiration must never
    /// settle a dispatched effect.
    async fn begin(&self, expected: &Checkpoint) -> Result<Checkpoint, FundingError>;
    async fn settle(
        &self,
        expected: &Checkpoint,
        effect: Effect,
    ) -> Result<Checkpoint, FundingError>;
    async fn advance(
        &self,
        expected: &Checkpoint,
        next: &Reservation,
    ) -> Result<Checkpoint, FundingError>;
}

/// Boundary to an adapter able to constrain what this credential can spend
/// before the billable request leaves the model gateway.
#[async_trait]
pub trait FundedModelGateway: Send + Sync {
    type Request: Send;

    async fn certify(&self, selected: &CredentialBinding) -> Result<PayerBinding, FundingError>;
    fn digest(&self, request: &Self::Request) -> [u8; 32];

    /// Enforce the reservation's payer, limit and stable request ID at the
    /// outbound boundary. An unconfirmed outcome stays in flight.
    async fn execute(
        &self,
        permit: &Checkpoint,
        request: Self::Request,
    ) -> Result<Effect, FundingError>;

    /// An indeterminate charge or external effect remains an error.
    async fn reconcile(&self, permit: &Checkpoint) -> Result<Effect, FundingError>;
}

#[async_trait]
pub trait ContinuationAuthority: Send + Sync {
    /// Revalidate the caller's authority over the turn and exact checkpoint.
    async fn authorize(
        &self,
        caller: &crate::Caller,
        checkpoint: &Checkpoint,
    ) -> Result<(), FundingError>;
}

pub struct ModelCredit<'a, A: ?Sized, G: ?Sized> {
    authority: &'a A,
    gateway: &'a G,
}

impl<'a, A: ReservationAuthority + ?Sized, G: FundedModelGateway + ?Sized> ModelCredit<'a, A, G> {
    pub fn new(authority: &'a A, gateway: &'a G) -> Self {
        Self { authority, gateway }
    }

    pub async fn reserve(
        &self,
        turn: String,
        selected: &CredentialBinding,
        limit: NonZeroU64,
        request: &G::Request,
    ) -> Result<Checkpoint, FundingError> {
        let request = Reservation {
            id: PhaseId { turn, phase: 0 },
            binding: self.binding(selected).await?,
            limit,
            request_digest: self.gateway.digest(request),
        };
        let held = self.authority.reserve_first(&request).await?;
        require_reservation(&held, &request)?;
        Ok(held)
    }

    pub async fn dispatch(
        &self,
        id: &PhaseId,
        selected: &CredentialBinding,
        request: G::Request,
    ) -> Result<Checkpoint, FundingError> {
        let held = self.check_dispatch(id, selected, &request).await?;
        if matches!(held.state, PhaseState::Settled(_)) {
            return Ok(held);
        }
        let permit = self.authority.begin(&held).await?;
        require_transition(&held, &permit, PhaseState::InFlight)?;
        let effect = self
            .gateway
            .execute(&permit, request)
            .await
            .map_err(|_| FundingError::ReconciliationRequired)?;
        self.settle(&permit, effect)
            .await
            .map_err(|_| FundingError::ReconciliationRequired)
    }

    async fn check_dispatch(
        &self,
        id: &PhaseId,
        selected: &CredentialBinding,
        request: &G::Request,
    ) -> Result<Checkpoint, FundingError> {
        let held = self.authority.read(id).await?;
        if held.reservation.id != *id {
            return Err(FundingError::CheckpointChanged);
        }
        if held.reservation.binding != self.binding(selected).await? {
            return Err(FundingError::BindingChanged);
        }
        if held.reservation.request_digest != self.gateway.digest(request) {
            return Err(FundingError::CheckpointChanged);
        }
        if held.state == PhaseState::InFlight {
            return Err(FundingError::ReconciliationRequired);
        }
        Ok(held)
    }

    pub async fn reconcile(&self, id: &PhaseId) -> Result<Checkpoint, FundingError> {
        let prior = self.authority.read(id).await?;
        if prior.reservation.id != *id {
            return Err(FundingError::CheckpointChanged);
        }
        match prior.state {
            PhaseState::InFlight => {
                let effect = self.gateway.reconcile(&prior).await?;
                self.settle(&prior, effect).await
            }
            PhaseState::Settled(_) => Ok(prior),
            PhaseState::Reserved => Err(FundingError::CheckpointChanged),
        }
    }

    /// The application must first reauthorize this exact funded checkpoint.
    /// A caller reusing an already resumed version cannot reserve another phase.
    pub async fn continue_after(
        &self,
        authority: &dyn ContinuationAuthority,
        caller: &crate::Caller,
        checkpoint: &Checkpoint,
        selected: &CredentialBinding,
        limit: NonZeroU64,
        request: &G::Request,
    ) -> Result<Checkpoint, FundingError> {
        authority.authorize(caller, checkpoint).await?;
        if !matches!(checkpoint.state, PhaseState::Settled(_)) {
            return Err(FundingError::ReconciliationRequired);
        }
        let binding = self.binding(selected).await?;
        if checkpoint.reservation.binding != binding {
            return Err(FundingError::BindingChanged);
        }
        let phase = checkpoint
            .reservation
            .id
            .phase
            .checked_add(1)
            .ok_or(FundingError::CheckpointChanged)?;
        let request = Reservation {
            id: PhaseId {
                turn: checkpoint.reservation.id.turn.clone(),
                phase,
            },
            binding,
            limit,
            request_digest: self.gateway.digest(request),
        };
        let next = self.authority.advance(checkpoint, &request).await?;
        require_reservation(&next, &request)?;
        if next.state != PhaseState::Reserved || next.version <= checkpoint.version {
            return Err(FundingError::CheckpointChanged);
        }
        Ok(next)
    }

    async fn binding(&self, selected: &CredentialBinding) -> Result<PayerBinding, FundingError> {
        let binding = self.gateway.certify(selected).await?;
        if binding.credential != *selected
            || binding.payer.is_empty()
            || binding.pool.is_empty()
            || selected.route.is_empty()
        {
            return Err(FundingError::BindingChanged);
        }
        Ok(binding)
    }

    async fn settle(
        &self,
        permit: &Checkpoint,
        effect: Effect,
    ) -> Result<Checkpoint, FundingError> {
        if matches!(effect, Effect::Finished { spent } if spent > permit.reservation.limit.get()) {
            return Err(FundingError::ReconciliationRequired);
        }
        let settled = self.authority.settle(permit, effect).await?;
        require_transition(permit, &settled, PhaseState::Settled(effect))?;
        Ok(settled)
    }
}

fn require_reservation(held: &Checkpoint, request: &Reservation) -> Result<(), FundingError> {
    if held.reservation != *request {
        return Err(FundingError::CheckpointChanged);
    }
    Ok(())
}

fn require_transition(
    before: &Checkpoint,
    after: &Checkpoint,
    state: PhaseState,
) -> Result<(), FundingError> {
    require_reservation(after, &before.reservation)?;
    if after.version <= before.version || after.state != state {
        return Err(FundingError::CheckpointChanged);
    }
    Ok(())
}

#[cfg(test)]
#[path = "model_credit_tests.rs"]
pub(in crate::sandbox) mod tests;
