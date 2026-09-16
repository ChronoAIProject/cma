use crate::ids::{AgentCommandId, AgentResponseId};
use crate::resource_access::{
    AccessPurpose, CallerContext, ResourceAccessError, ResourceCapability, ResourceTarget,
};
use crate::sandbox::AgentOperationDelivery;
use crate::AgentId;

use super::admission_service::bounded;
use super::*;

/// Safe public acceptance facts. No input, actor, owner, source reference,
/// credential/grant identifier or runtime identity is copied from the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReceiptView {
    pub command_id: AgentCommandId,
    pub agent_id: AgentId,
    pub response_id: Option<AgentResponseId>,
    pub admitted_at: String,
    pub agent_order: u64,
    pub event_cursor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStatusView {
    pub receipt: CommandReceiptView,
    pub status: AgentResponseStatus,
    pub queue_deadline: String,
    pub turn_sequence: Option<u64>,
    pub observed_cursor: Option<u64>,
    pub delivery: Option<AgentOperationDelivery>,
    pub failure: Option<CommandFailureCode>,
}

/// Authorized projection boundaries from the exact admitted command. The
/// operation cursor distinguishes a control's dispatch from earlier activity
/// in the same turn, without exposing runtime or credential identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProjectionView {
    pub status: CommandStatusView,
    pub operation_sequence: Option<u64>,
}

impl CommandStatusView {
    pub(super) fn from_record(record: &AdmittedAgentCommand) -> Self {
        let receipt = &record.receipt;
        Self {
            receipt: CommandReceiptView {
                command_id: receipt.envelope.command_id.clone(),
                agent_id: receipt.envelope.agent_id.clone(),
                response_id: receipt.response_id.clone(),
                admitted_at: receipt.admitted_at.clone(),
                agent_order: receipt.agent_order,
                event_cursor: receipt.event_cursor,
            },
            status: record.status,
            queue_deadline: record.queue_deadline.clone(),
            turn_sequence: record
                .work
                .dispatch
                .as_ref()
                .map(|dispatch| dispatch.turn_sequence),
            observed_cursor: record
                .work
                .observation
                .as_ref()
                .map(|observation| observation.event_cursor),
            delivery: record
                .work
                .observation
                .as_ref()
                .map(|observation| observation.delivery)
                .or_else(|| {
                    record
                        .work
                        .dispatch
                        .as_ref()
                        .map(|dispatch| dispatch.delivery)
                }),
            failure: record.work.failure.as_ref().map(|failure| failure.code),
        }
    }
}

impl AgentCommandService {
    pub async fn command_status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentCommandId,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        let record = bounded(self.store.get_command(id))
            .await??
            .ok_or(CommandLedgerError::NotFound)?;
        if &record.receipt.envelope.command_id != id {
            return Err(CommandLedgerError::Corrupt.into());
        }
        self.authorized_record(context, credential, record)
            .await
            .map(|record| CommandStatusView::from_record(&record))
    }

    pub async fn command_projection(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentCommandId,
    ) -> Result<CommandProjectionView, CommandAdmissionError> {
        let record = bounded(self.store.get_command(id))
            .await??
            .ok_or(CommandLedgerError::NotFound)?;
        if &record.receipt.envelope.command_id != id {
            return Err(CommandLedgerError::Corrupt.into());
        }
        let record = self.authorized_record(context, credential, record).await?;
        Ok(CommandProjectionView {
            status: CommandStatusView::from_record(&record),
            operation_sequence: record
                .work
                .dispatch
                .as_ref()
                .map(|value| value.operation_sequence),
        })
    }

    pub async fn response_status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentResponseId,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        let record = bounded(self.store.get_response_command(id))
            .await??
            .ok_or(CommandLedgerError::NotFound)?;
        if record.receipt.response_id.as_ref() != Some(id) {
            return Err(CommandLedgerError::Corrupt.into());
        }
        self.authorized_record(context, credential, record)
            .await
            .map(|record| CommandStatusView::from_record(&record))
    }

    async fn authorized_record(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        original: AdmittedAgentCommand,
    ) -> Result<AdmittedAgentCommand, CommandAdmissionError> {
        original.validate()?;
        let target = ResourceTarget::Agent {
            agent_id: original.receipt.envelope.agent_id.clone(),
        };
        let capability = ResourceCapability::ViewHistory;
        let authorized = bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await??;
        if original.receipt.envelope.owner
            != authorized
                .snapshot
                .scope(&target)
                .map_err(ResourceAccessError::from)?
                .owner
        {
            return Err(CommandLedgerError::NotFound.into());
        }
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store
                    .get_command(&original.receipt.envelope.command_id),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let current = result??.ok_or(CommandLedgerError::NotFound)?;
        current.validate()?;
        if current.receipt != original.receipt
            || current.key_sha256 != original.key_sha256
            || current.target_turn != original.target_turn
        {
            return Err(CommandLedgerError::Corrupt.into());
        }
        Ok(current)
    }
}
