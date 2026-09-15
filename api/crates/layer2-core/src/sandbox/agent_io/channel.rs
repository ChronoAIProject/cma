//! Trusted Channel admission into the common Agent ledger. An accepted message
//! and retained allocation are historical evidence; only the opaque invoke proof
//! plus the store's atomic current fences permit reservation.

use std::{sync::Arc, time::Duration};

use crate::bridge::{
    allocation::AllocationRecord,
    ingress::StoredInbox,
    invoke::{BridgeInvokeError, BridgeInvokeProof},
    partition::{PartitionError, PartitionRefusal},
};

use super::{AdmittedAgentCommand, CommandLedgerError, CommandReservation};

mod execution;
mod refusal;
pub use refusal::{ChannelHoldRefusal, ChannelHoldRefusalReason};
mod prepare;
mod sidecar;
pub use execution::{
    ChannelCommandExecutionSource, ChannelExecutionAuthorization, ChannelExecutionFence,
};
pub use sidecar::{ChannelCommandHold, ChannelCommandSidecar};

impl CommandReservation {
    /// The generic UI/API path has no current bridge-grant transaction fence.
    /// Channel attribution must use the opaque Channel reservation port.
    pub fn require_non_channel(&self) -> Result<(), CommandLedgerError> {
        if matches!(self.envelope.actor, super::AgentActor::Channel { .. })
            || self.envelope.source.kind == super::AgentSourceKind::Channel
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelCommandAdmissionError {
    #[error(transparent)]
    Invoke(#[from] BridgeInvokeError),
    #[error(transparent)]
    Partition(#[from] PartitionError),
    #[error(transparent)]
    Ledger(#[from] CommandLedgerError),
    #[error(transparent)]
    Input(#[from] crate::sandbox::AgentOperationError),
    #[error("channel message is not admitted: {0:?}")]
    Refused(PartitionRefusal),
    #[error("channel message or allocation evidence does not match")]
    SourceMismatch,
    #[error("channel command admission is unavailable")]
    Unavailable,
}

/// No public fields or deserialization. Input and identity are derived from the
/// exact persisted inbox, never accepted separately from its message evidence.
#[derive(Clone)]
pub struct ChannelCommandReservation {
    proof: BridgeInvokeProof,
    inbox: StoredInbox,
    allocation: AllocationRecord,
    reservation: CommandReservation,
}

impl ChannelCommandReservation {
    pub fn proof(&self) -> &BridgeInvokeProof {
        &self.proof
    }
    pub fn inbox(&self) -> &StoredInbox {
        &self.inbox
    }
    pub fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub fn reservation(&self) -> &CommandReservation {
        &self.reservation
    }

    pub fn require_current(&self) -> Result<(), ChannelCommandAdmissionError> {
        self.proof.require_current()?;
        self.reservation.validate()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ChannelCommandAdmissionError::Unavailable)?;
        // Callback retention bounds new admission. An already admitted command
        // retains its original source and needs fresh exact execution authority,
        // even after the transport inbox has expired or been removed by TTL.
        let new_admission = match self.proof.effect() {
            crate::bridge::invoke::BridgeInvokeEffect::AdmitMessage { inbox_id }
                if inbox_id == &self.inbox.id =>
            {
                true
            }
            crate::bridge::invoke::BridgeInvokeEffect::ExecuteCommand { command_id }
                if command_id == &self.reservation.envelope.command_id =>
            {
                false
            }
            _ => return Err(ChannelCommandAdmissionError::SourceMismatch),
        };
        if new_admission && self.inbox.expires_at <= now.as_secs().min(i64::MAX as u64) as i64 {
            return Err(ChannelCommandAdmissionError::Unavailable);
        }
        Ok(())
    }

    /// Called under the reservation transaction before replay or insertion.
    /// The adapter additionally fences current invoke, custody and resource state.
    pub fn require_records(
        &self,
        inbox: &StoredInbox,
        allocation: &AllocationRecord,
    ) -> Result<(), ChannelCommandAdmissionError> {
        self.require_current()?;
        if inbox != &self.inbox || allocation != &self.allocation {
            return Err(ChannelCommandAdmissionError::SourceMismatch);
        }
        Ok(())
    }

    pub fn require_result(
        &self,
        record: &AdmittedAgentCommand,
    ) -> Result<(), ChannelCommandAdmissionError> {
        record.validate()?;
        let retained = &record.receipt.envelope;
        let expected = &self.reservation.envelope;
        if retained.command_id != expected.command_id
            || retained.agent_id != expected.agent_id
            || retained.owner != expected.owner
            || retained.actor != expected.actor
            || retained.source != expected.source
            || retained.correlation_id != expected.correlation_id
            || retained.command != expected.command
            || retained.request_fingerprint != expected.request_fingerprint
            || retained.authorization.authority_id != expected.authorization.authority_id
            || record.key_sha256 != self.reservation.key_sha256
            || record.receipt.response_id != self.reservation.response_id
        {
            return Err(CommandLedgerError::Corrupt.into());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait ChannelCommandStore: Send + Sync {
    /// Uses the same command collection and per-Agent order as UI/API commands.
    /// Current bridge, grant, custody, target, inbox and allocation fences precede
    /// BOTH replay and insertion within the same reservation transaction.
    async fn reserve_channel_command(
        &self,
        request: ChannelCommandReservation,
    ) -> Result<AdmittedAgentCommand, ChannelCommandAdmissionError>;

    async fn channel_execution_source(
        &self,
        _command: &AdmittedAgentCommand,
    ) -> Result<ChannelCommandExecutionSource, ChannelCommandAdmissionError> {
        Err(ChannelCommandAdmissionError::Unavailable)
    }

    /// Compare the exact held record and sidecar, with all current source and
    /// invoke fences in the same transaction. This grants no later dispatch.
    async fn release_channel_command(
        &self,
        _fence: Arc<ChannelExecutionFence>,
        _command: &AdmittedAgentCommand,
    ) -> Result<(), ChannelCommandAdmissionError> {
        Err(ChannelCommandAdmissionError::Unavailable)
    }

    async fn refuse_channel_hold(
        &self,
        _refusal: Arc<ChannelHoldRefusal>,
    ) -> Result<(), ChannelCommandAdmissionError> {
        Err(ChannelCommandAdmissionError::Unavailable)
    }
}

pub struct ChannelAgentCommandService {
    store: Arc<dyn ChannelCommandStore>,
}

impl ChannelAgentCommandService {
    pub fn new(store: Arc<dyn ChannelCommandStore>) -> Self {
        Self { store }
    }

    /// A held durable command is not permission to execute, revive, approve or
    /// send a platform reply. The common worker must obtain separate authority.
    pub async fn admit(
        &self,
        proof: BridgeInvokeProof,
        inbox: StoredInbox,
        allocation: AllocationRecord,
    ) -> Result<AdmittedAgentCommand, ChannelCommandAdmissionError> {
        let request = ChannelCommandReservation::prepare(proof, inbox, allocation)?;
        let deadline = request
            .proof()
            .deadline()
            .min(tokio::time::Instant::now() + Duration::from_secs(15));
        let result = tokio::time::timeout_at(
            deadline,
            self.store.reserve_channel_command(request.clone()),
        )
        .await
        .map_err(|_| ChannelCommandAdmissionError::Unavailable)??;
        request.require_current()?;
        request.require_result(&result)?;
        Ok(result)
    }
}
