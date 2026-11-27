//! Additive resource execution protocol. Decoded messages are untrusted data;
//! manager ownership and a current committed registration remain independent
//! prerequisites. Legacy owner messages and command DTOs are unchanged.

mod identity;
mod receipts;
pub mod sender;
pub use identity::*;
pub use receipts::*;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CONTROL_ROUTE_PREFIX: &str = "/runtime/resource-execution/v1";
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;
pub const MAX_LEASE_MS: u64 = 3_000;
pub const RENEW_AFTER_MS: u64 = 1_000;
pub const STOP_BOUND_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("resource execution protocol is unsupported")]
    Unsupported,
    #[error("invalid resource execution identity")]
    InvalidIdentity,
    #[error("resource execution audience does not match")]
    AudienceMismatch,
    #[error("resource execution evidence does not match")]
    EvidenceMismatch,
    #[error("resource execution lease expired")]
    Expired,
    #[error("resource execution capacity reached")]
    Capacity,
    #[error("resource execution outcome is unknown")]
    Unknown,
    #[error("resource execution transport is unavailable")]
    Unavailable,
}

pub type ProtocolResult<T> = Result<T, ProtocolError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOperation {
    Prompt,
    RecoverablePrompt,
    Steer,
    PermissionAnswer,
    Configure,
    ContextChange,
}

impl ExecutionOperation {
    pub fn capability(self) -> ProtectedCapability {
        match self {
            Self::Prompt | Self::RecoverablePrompt | Self::Steer => ProtectedCapability::Prompt,
            Self::PermissionAnswer => ProtectedCapability::ResolveApproval,
            Self::Configure | Self::ContextChange => ProtectedCapability::OperateAgent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerKey {
    pub instance: OpaqueId,
    pub epoch: Generation,
    pub agent_id: OpaqueId,
}

impl ManagerKey {
    pub fn matches_owner(&self, owner: &crate::owner_protocol::TransportOwner) -> bool {
        owner.validate().is_ok()
            && self.instance.as_str() == owner.instance_id
            && self.epoch.get() == owner.epoch
            && self.agent_id.as_str() == owner.agent_id
    }
}

/// Both clock values belong to this receiver boot's suspend-aware clock.
/// Delivery time must never be used to recompute the original deadline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverChallengeV1 {
    pub protocol: u32,
    pub receiver: ReceiverKey,
    pub nonce: OpaqueId,
    pub created_tick_ms: u64,
    pub deadline_tick_ms: u64,
}

impl ReceiverChallengeV1 {
    pub fn validate(&self) -> ProtocolResult<()> {
        version(self.protocol)?;
        if !self
            .deadline_tick_ms
            .checked_sub(self.created_tick_ms)
            .is_some_and(|duration| duration > 0 && duration <= MAX_LEASE_MS)
        {
            return Err(ProtocolError::InvalidIdentity);
        }
        Ok(())
    }

    pub fn check_at(&self, receiver: &ReceiverKey, tick_ms: u64) -> ProtocolResult<()> {
        self.validate()?;
        if &self.receiver != receiver {
            return Err(ProtocolError::AudienceMismatch);
        }
        if tick_ms < self.created_tick_ms || tick_ms >= self.deadline_tick_ms {
            return Err(ProtocolError::Expired);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTicketV1 {
    pub protocol: u32,
    pub execution: ExecutionKey,
    pub basis: ExecutionBasisV1,
    pub operation: ExecutionOperation,
    pub input_digest: Digest,
    pub manager: ManagerKey,
    pub challenge: ReceiverChallengeV1,
    pub nonce: OpaqueId,
    pub registration: Digest,
}

impl ExecutionTicketV1 {
    pub fn validate(&self) -> ProtocolResult<()> {
        version(self.protocol)?;
        self.challenge.validate()?;
        MemberKey::Execution {
            execution: self.execution.clone(),
        }
        .validate()?;
        if self.execution.receiver != self.challenge.receiver
            || self.execution.agent_id != self.manager.agent_id
        {
            return Err(ProtocolError::AudienceMismatch);
        }
        self.basis.validate()?;
        if self.basis.capability() != self.operation.capability() {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }

    /// Structural matching after the control transport authenticates the
    /// owner. This does not consume a nonce or open a receiver's start gate.
    pub fn validate_for(
        &self,
        owner: &crate::owner_protocol::TransportOwner,
        execution: &ExecutionKey,
        input_digest: &Digest,
        tick_ms: u64,
    ) -> ProtocolResult<()> {
        self.validate()?;
        if !self.manager.matches_owner(owner)
            || &self.execution != execution
            || &self.input_digest != input_digest
        {
            return Err(ProtocolError::AudienceMismatch);
        }
        self.challenge.check_at(&execution.receiver, tick_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationV1 {
    pub protocol: u32,
    pub execution: ExecutionKey,
    pub registration: Digest,
    pub basis: ExecutionBasisV1,
    pub manager: ManagerKey,
    pub challenge: ReceiverChallengeV1,
    pub sequence: Generation,
}

impl ContinuationV1 {
    pub fn validate_for(&self, original: &ExecutionTicketV1) -> ProtocolResult<()> {
        version(self.protocol)?;
        original.validate()?;
        self.challenge.validate()?;
        if self.execution != original.execution
            || self.registration != original.registration
            || self.basis != original.basis
            || self.challenge.receiver != original.execution.receiver
            || self.manager.agent_id != original.execution.agent_id
            || self.challenge.nonce == original.challenge.nonce
            || self.challenge.created_tick_ms <= original.challenge.created_tick_ms
            || self.challenge.deadline_tick_ms <= original.challenge.deadline_tick_ms
            || self.sequence.get() <= 1
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }

    /// A receiver compares against its retained last lease before consuming
    /// the next reply. An old response cannot refresh its own deadline.
    pub fn validate_after(&self, previous: &Self) -> ProtocolResult<()> {
        version(self.protocol)?;
        version(previous.protocol)?;
        self.challenge.validate()?;
        previous.challenge.validate()?;
        if self.execution != previous.execution
            || self.registration != previous.registration
            || self.basis != previous.basis
            || self.challenge.receiver != previous.challenge.receiver
            || self.sequence != previous.sequence.successor()?
            || self.challenge.nonce == previous.challenge.nonce
            || self.challenge.created_tick_ms <= previous.challenge.created_tick_ms
            || self.challenge.deadline_tick_ms <= previous.challenge.deadline_tick_ms
            || self.manager.agent_id != self.execution.agent_id
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

/// A capability is advertised by an implementation only after native start,
/// containment and egress qualification. A DTO alone proves none of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionCapabilities {
    pub protocol: u32,
    pub targeted_native_stop: bool,
    pub mediated_effects: bool,
    pub mixed_principal_control: bool,
}

#[async_trait::async_trait]
pub trait RuntimeExecutionControl: Send + Sync {
    fn capabilities(&self) -> Option<ExecutionCapabilities> {
        None
    }
    async fn challenge(&self, _execution: &ExecutionKey) -> ProtocolResult<ReceiverChallengeV1> {
        Err(ProtocolError::Unsupported)
    }
    async fn admit_start(&self, _ticket: &ExecutionTicketV1) -> ProtocolResult<StartObservationV1> {
        Err(ProtocolError::Unsupported)
    }
    async fn continue_execution(&self, _lease: &ContinuationV1) -> ProtocolResult<()> {
        Err(ProtocolError::Unsupported)
    }
    async fn install_barrier(
        &self,
        _barrier: &BarrierRequestV1,
    ) -> ProtocolResult<BarrierReceiptV1> {
        Err(ProtocolError::Unsupported)
    }
    async fn cancel(&self, _barrier: &BarrierRequestV1) -> ProtocolResult<StopReceiptV1> {
        Err(ProtocolError::Unsupported)
    }
    async fn stop_receipt(
        &self,
        _barrier: &BarrierRequestV1,
    ) -> ProtocolResult<Option<StopReceiptV1>> {
        Err(ProtocolError::Unsupported)
    }
}

#[derive(Debug, Default)]
pub struct UnsupportedExecutionControl;
impl RuntimeExecutionControl for UnsupportedExecutionControl {}

pub fn version(value: u32) -> ProtocolResult<()> {
    if value == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError::Unsupported)
    }
}

pub fn decode_message<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> ProtocolResult<T> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::Capacity);
    }
    serde_json::from_slice(bytes).map_err(|_| ProtocolError::InvalidIdentity)
}

#[cfg(test)]
mod tests;
